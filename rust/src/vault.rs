//! Safe access to the files of the notes vault (port of internal/vault).
//!
//! Every access goes through a `cap_std::fs::Dir`, the equivalent of Go's `os.Root`: paths
//! coming from the browser cannot escape the vault directory (`../`, symlinks, absolute
//! paths), the kernel resolves them relative to an open directory descriptor.
//!
//! The filesystem calls are blocking, so the async wrappers run them off the async workers:
//! reads, listings, creates and image saves on Tokio's blocking pool; the operations that must
//! be serialised (save, move, trash: they check-then-write) on ONE dedicated writer thread fed
//! by a queue. A single consumer gives the mutual exclusion of Go's `sync.Mutex` with FIFO
//! fairness and without any lock hand-off: an earlier version took an async mutex and then hopped
//! to the blocking pool, and under 1000 contending clients the critical section itself waited in
//! the pool's queue behind hundreds of reads (see bench/RESULTS.md).

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use cap_std::ambient_authority;
use cap_std::fs::{
    Dir, DirBuilder, DirBuilderExt, File, Metadata, MetadataExt, OpenOptions, OpenOptionsExt,
};
use serde::{Serialize, Serializer};
use std::os::unix::fs::PermissionsExt;

use crate::golike;

/// The vault errors, with the same texts as the Go sentinel errors.
#[derive(Debug)]
pub enum Error {
    /// The file changed on disk; carries the version found there.
    Conflict(Box<Note>),
    InvalidPath,
    NotNote,
    Exists,
    BadName,
    NotImage,
    Io {
        op: &'static str,
        path: String,
        source: io::Error,
    },
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Conflict(_) => f.write_str("file was modified on disk"),
            Error::InvalidPath => f.write_str("invalid path"),
            Error::NotNote => f.write_str("only .md files can be edited"),
            Error::Exists => f.write_str("a file with this name already exists"),
            Error::BadName => f.write_str(
                "the name cannot contain # ^ [ ] | (Obsidian rejects them; they would break links)",
            ),
            Error::NotImage => f.write_str("the file is not a PNG, JPEG, GIF, WebP or BMP image"),
            Error::Io { op, path, source } => {
                if is_escape(source) {
                    write!(f, "{op} {path}: path escapes from parent")
                } else {
                    write!(f, "{op} {path}: {}", go_errno(source))
                }
            }
            Error::Internal(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

/// cap-std reports an escape (`..`, absolute symlink) as `PermissionDenied` without an errno.
fn is_escape(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied && e.raw_os_error().is_none()
}

/// Go's wording for the common errnos (`syscall.Errno.Error()`), lower case.
pub fn go_errno(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(1) => "operation not permitted".into(),
        Some(2) => "no such file or directory".into(),
        Some(13) => "permission denied".into(),
        Some(17) => "file exists".into(),
        Some(18) => "invalid cross-device link".into(),
        Some(20) => "not a directory".into(),
        Some(21) => "is a directory".into(),
        Some(22) => "invalid argument".into(),
        Some(28) => "no space left on device".into(),
        Some(36) => "file name too long".into(),
        Some(39) => "directory not empty".into(),
        Some(40) => "too many levels of symbolic links".into(),
        _ => e.to_string(),
    }
}

impl Error {
    pub fn io(op: &'static str, path: &str, source: io::Error) -> Error {
        Error::Io {
            op,
            path: path.to_string(),
            source,
        }
    }

    /// `errors.Is(err, fs.ErrNotExist)`.
    pub fn is_not_exist(&self) -> bool {
        matches!(self, Error::Io { source, .. } if source.kind() == io::ErrorKind::NotFound)
    }

    /// The error of a path that escapes the vault through a symlink.
    pub fn is_escape(&self) -> bool {
        matches!(self, Error::Io { source, .. } if is_escape(source))
    }
}

/// Extensions listed in the tree. Images are included so the preview can resolve
/// `![[photo.png]]` by name, like Obsidian.
const VISIBLE_EXT: &[&str] = &[
    ".md", ".pdf", ".epub", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".avif", ".bmp",
];

/// Maximum size of a pasted image.
pub const MAX_ATTACHMENT: usize = 20 << 20;

/// The vault trash, the same one Obsidian uses.
pub const TRASH_DIR: &str = ".trash";

/// The blocking half of the vault: every filesystem operation, synchronously.
#[derive(Debug)]
pub struct Core {
    dir: PathBuf,
    root: Dir,
    /// Give every accepted save a strictly newer mtime than the version it replaces (see
    /// `save_locked`). On by default; `set_strict_mtime(false)` restores the Go behaviour.
    strict_mtime: bool,
}

type Job = Box<dyn FnOnce(&Core) + Send>;

/// The vault as the API sees it: the blocking `Core` plus the writer thread's queue.
#[derive(Debug)]
pub struct Vault {
    core: Arc<Core>,
    writer: mpsc::Sender<Job>,
}

impl std::ops::Deref for Vault {
    type Target = Core;
    fn deref(&self) -> &Core {
        &self.core
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Entry>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Note {
    pub path: String,
    pub content: String,
    /// UnixNano; a string in JSON (it exceeds 2^53, the exact limit of a JS Number).
    #[serde(serialize_with = "ser_i64_string")]
    pub mtime: i64,
}

fn ser_i64_string<S: Serializer>(v: &i64, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

/// Whether the (relative) path goes through a hidden file or directory, such as
/// `.obsidian`, `.stversions` or the temporary write files.
pub fn hidden(p: &str) -> bool {
    p.split('/')
        .any(|part| part.starts_with('.') && part != ".")
}

/// Whether the last element of `p` can be referenced in a `[[link]]`.
pub fn valid_name(p: &str) -> bool {
    !golike::base(p).contains(['#', '^', '[', ']', '|', '\r', '\n'])
}

pub fn clean(p: &str) -> Result<String> {
    let p = golike::clean(p.strip_prefix('/').unwrap_or(p));
    if p == "." || !golike::valid_path(&p) || hidden(&p) {
        return Err(Error::InvalidPath);
    }
    Ok(p)
}

fn is_md(p: &str) -> bool {
    golike::ext(p).eq_ignore_ascii_case(".md")
}

fn mtime_nanos(md: &Metadata) -> i64 {
    md.mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(md.mtime_nsec())
}

fn rand_hex() -> String {
    let mut b = [0u8; 4];
    // A failure of the OS random source is not recoverable and not worth failing a save for:
    // the name only has to avoid collisions (it is created with O_EXCL anyway).
    if getrandom::fill(&mut b).is_err() {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        b = t.to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Vault {
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Vault> {
        Self::open_with(dir, true)
    }

    /// `strict_mtime`: give every accepted save a strictly newer mtime than the version it
    /// replaces (`false` restores Go's behaviour; used by the benchmark to price the guard).
    pub fn open_with(dir: impl AsRef<Path>, strict_mtime: bool) -> io::Result<Vault> {
        let dir = dir.as_ref().to_path_buf();
        let root = Dir::open_ambient_dir(&dir, ambient_authority())?;
        let core = Arc::new(Core {
            dir,
            root,
            strict_mtime,
        });
        let (tx, rx) = mpsc::channel::<Job>();
        let worker = Arc::clone(&core);
        std::thread::Builder::new()
            .name("vault-writer".into())
            .spawn(move || {
                // Ends when the `Vault` (the only sender) is dropped.
                for job in rx {
                    // A panicking job must not take the writer down with it: its reply channel
                    // is dropped, which the caller reports as an internal error.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&worker)));
                }
            })?;
        Ok(Vault { core, writer: tx })
    }
}

impl Core {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    // ---- blocking primitives -------------------------------------------------------

    fn mkdir_all(&self, p: &str) -> Result<()> {
        let mut b = DirBuilder::new();
        b.recursive(true).mode(0o755);
        self.root
            .create_dir_with(p, &b)
            .map_err(|e| self.mkdir_error(p, e))
    }

    /// Words the failure of `mkdir -p` like Go's `os.Root.MkdirAll`: `openat <prefix>` when a
    /// path element is not a directory, `mkdirat <path>` otherwise.
    fn mkdir_error(&self, p: &str, e: io::Error) -> Error {
        if is_escape(&e) {
            return Error::io("mkdirat", p, e);
        }
        let comps: Vec<&str> = p.split('/').collect();
        let mut prefix = String::new();
        for (i, c) in comps.iter().enumerate() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(c);
            let Ok(md) = self.root.symlink_metadata(&prefix) else {
                break;
            };
            if md.is_dir() || md.file_type().is_symlink() {
                continue;
            }
            return if i + 1 == comps.len() {
                Error::io("mkdirat", &prefix, io::Error::from_raw_os_error(17))
            } else {
                Error::io("openat", &prefix, io::Error::from_raw_os_error(20))
            };
        }
        Error::io("mkdirat", p, e)
    }

    /// `renameat2(RENAME_NOREPLACE)` confined to the vault: `Exists` if the destination appeared
    /// since the caller looked. (Go's `os.Root` has no such call, so a file created in that window
    /// is silently overwritten there.) Filesystems without support fall back to a plain rename.
    fn rename_noreplace(&self, from: &str, to: &str) -> Result<()> {
        use rustix::fs::{RenameFlags, renameat_with};
        let open_parent = |p: &str| -> io::Result<Dir> {
            match golike::dir(p).as_str() {
                "." => self.root.try_clone(),
                d => self.root.open_dir(d),
            }
        };
        let (fd, td) = (
            open_parent(from).map_err(|e| Error::io("renameat", from, e))?,
            open_parent(to).map_err(|e| Error::io("renameat", to, e))?,
        );
        let (fname, tname) = (golike::base(from), golike::base(to));
        match renameat_with(&fd, fname, &td, tname, RenameFlags::NOREPLACE) {
            Ok(()) => Ok(()),
            Err(rustix::io::Errno::EXIST) => Err(Error::Exists),
            Err(rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS) => self
                .root
                .rename(from, &self.root, to)
                .map_err(|e| Error::io("renameat", from, e)),
            Err(e) => Err(Error::io("renameat", from, io::Error::from(e))),
        }
    }

    /// `Exists` of the Go version: does the path exist in the vault (symlinks not followed)?
    pub fn exists_sync(&self, p: &str) -> bool {
        match clean(p) {
            Ok(p) => self.root.symlink_metadata(&p).is_ok(),
            Err(_) => false,
        }
    }

    /// The visible directories and files, directories first.
    pub fn tree_sync(&self) -> Result<Vec<Entry>> {
        self.read_dir(".")
    }

    fn read_dir(&self, dir: &str) -> Result<Vec<Entry>> {
        let rd = self
            .root
            .read_dir(dir)
            .map_err(|e| Error::io("openat", dir, e))?;
        let mut items: Vec<(String, bool)> = Vec::new();
        for it in rd {
            let it = it.map_err(|e| Error::io("readdir", dir, e))?;
            let is_dir = it
                .file_type()
                .map_err(|e| Error::io("readdir", dir, e))?
                .is_dir();
            items.push((it.file_name().to_string_lossy().into_owned(), is_dir));
        }
        // fs.ReadDir returns the entries sorted by file name; the stable sort below relies on it.
        items.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let mut entries = Vec::with_capacity(items.len());
        for (name, is_dir) in items {
            if name.starts_with('.') {
                continue;
            }
            let p = golike::join(&[dir, &name]);
            if is_dir {
                let children = self.read_dir(&p)?;
                entries.push(Entry {
                    name,
                    path: p,
                    dir: true,
                    children,
                });
            } else if VISIBLE_EXT.contains(&golike::ext(&name).to_lowercase().as_str()) {
                entries.push(Entry {
                    name,
                    path: p,
                    dir: false,
                    children: Vec::new(),
                });
            }
        }
        entries.sort_by_cached_key(|e| (!e.dir, e.name.to_lowercase()));
        Ok(entries)
    }

    pub fn read_sync(&self, p: &str) -> Result<Note> {
        let p = clean(p)?;
        let mut f = self.root.open(&p).map_err(|e| Error::io("openat", &p, e))?;
        let mut data = Vec::new();
        f.read_to_end(&mut data).map_err(|e| Error::Io {
            op: "read",
            path: self.dir.join(&p).to_string_lossy().into_owned(),
            source: e,
        })?;
        // The mtime comes from the same descriptor as the content (the Go version stats the
        // path afterwards, which can pair old content with a newer mtime).
        let md = f.metadata().map_err(|e| Error::io("statat", &p, e))?;
        Ok(Note {
            path: p,
            content: golike::lossy_per_byte(&data),
            mtime: mtime_nanos(&md),
        })
    }

    /// Atomic write (temporary file + rename). It only runs on the writer thread.
    ///
    /// `base_mtime` is the mtime the client read; `0` forces the write.
    fn save_locked(&self, p: &str, content: &str, base_mtime: i64) -> Result<Note> {
        let p = clean(p)?;
        if !is_md(&p) {
            return Err(Error::NotNote);
        }
        let info = match self.root.metadata(&p) {
            Ok(m) => Some(m),
            Err(e) if base_mtime == 0 && e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(Error::io("statat", &p, e)),
        };
        let mode = info.as_ref().map_or(0o644, |m| m.mode() & 0o777);
        if let Some(info) = &info
            && base_mtime != 0
            && mtime_nanos(info) != base_mtime
        {
            let current = self.read_sync(&p)?;
            return Err(Error::Conflict(Box::new(current)));
        }

        let dir = golike::dir(&p);
        if dir != "." {
            self.mkdir_all(&dir)?;
        }
        let tmp = golike::join(&[
            &dir,
            &format!(".{}.quirenote-{}.tmp", golike::base(&p), rand_hex()),
        ]);
        // Strictly newer than the version being replaced: the kernel stamps files with a coarse
        // clock (a few ms), so two saves in the same tick would get the *same* mtime and the
        // second client's stale `mtime` would still match. Go has no such guard (lost updates).
        let new_mtime = match (&info, self.strict_mtime) {
            (Some(m), true) => {
                let prev = std::time::UNIX_EPOCH
                    + std::time::Duration::new(
                        m.mtime().max(0) as u64,
                        m.mtime_nsec().max(0) as u32,
                    );
                let now = std::time::SystemTime::now();
                Some(now.max(prev + std::time::Duration::from_nanos(1)))
            }
            _ => None,
        };
        let written = (|| -> io::Result<()> {
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true).mode(mode);
            let mut f = self.root.open_with(&tmp, &opts)?.into_std();
            f.write_all(content.as_bytes())?;
            if info.is_some() {
                f.set_permissions(std::fs::Permissions::from_mode(mode))?;
            }
            if let Some(t) = new_mtime {
                f.set_modified(t)?;
            }
            Ok(())
        })();
        if let Err(e) = written {
            let _ = self.root.remove_file(&tmp);
            return Err(Error::io("openat", &tmp, e));
        }
        if let Err(e) = self.root.rename(&tmp, &self.root, &p) {
            let _ = self.root.remove_file(&tmp);
            return Err(Error::io("renameat", &p, e));
        }
        self.read_sync(&p)
    }

    /// Creates a folder (and its parents). Does not fail if it exists.
    pub fn mkdir_sync(&self, p: &str) -> Result<String> {
        let p = clean(p)?;
        if !valid_name(&p) {
            return Err(Error::BadName);
        }
        self.mkdir_all(&p)?;
        Ok(p)
    }

    /// Creates a new note; `Exists` if the file is already there, never overwriting.
    pub fn create_sync(&self, p: &str, content: &str) -> Result<Note> {
        let p = clean(p)?;
        if !is_md(&p) {
            return Err(Error::NotNote);
        }
        if !valid_name(&p) {
            return Err(Error::BadName);
        }
        let dir = golike::dir(&p);
        if dir != "." {
            self.mkdir_all(&dir)?;
        }
        self.create_excl(&p, content.as_bytes())?;
        self.read_sync(&p)
    }

    /// Writes a new file (O_EXCL) at an already validated path.
    fn create_excl(&self, p: &str, data: &[u8]) -> Result<()> {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true).mode(0o644);
        let mut f = match self.root.open_with(p, &opts) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(Error::Exists),
            Err(e) => return Err(Error::io("openat", p, e)),
        };
        if let Err(e) = f.write_all(data).and_then(|()| f.flush()) {
            drop(f);
            let _ = self.root.remove_file(p); // do not leave a truncated file that blocks retries
            return Err(Error::io("write", p, e));
        }
        Ok(())
    }

    /// Follows Obsidian's attachment setting (`.obsidian/app.json`, `attachmentFolderPath`).
    fn attachment_dir(&self, note: &str) -> String {
        let mut cfg_dir = String::new();
        if let Ok(mut f) = self.root.open(".obsidian/app.json") {
            let mut data = Vec::new();
            if f.read_to_end(&mut data).is_ok()
                && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&data)
                && let Some(s) = v.get("attachmentFolderPath").and_then(|s| s.as_str())
            {
                cfg_dir = s.to_string();
            }
        }
        let dir = cfg_dir.trim();
        if dir.is_empty() || dir == "/" {
            ".".into()
        } else if dir == "." || dir.starts_with("./") {
            let note_dir = clean(note).map_or_else(|_| ".".into(), |n| golike::dir(&n));
            golike::join(&[&note_dir, dir.strip_prefix('.').unwrap_or(dir)])
        } else {
            golike::clean(dir.trim_matches('/'))
        }
    }

    /// Writes a pasted/dropped image to the attachment folder, named like Obsidian does
    /// ("Pasted image YYYYMMDDhhmmss.png"), never overwriting anything.
    pub fn save_image_sync(&self, note: &str, data: &[u8]) -> Result<String> {
        if data.is_empty() || data.len() > MAX_ATTACHMENT {
            return Err(Error::NotImage);
        }
        let ext = sniff_image_ext(data).ok_or(Error::NotImage)?;
        let mut dir = self.attachment_dir(note);
        if dir != "." {
            let d = clean(&dir)?;
            self.mkdir_all(&d)?;
            dir = d;
        }
        let base = format!(
            "Pasted image {}",
            chrono::Local::now().format("%Y%m%d%H%M%S")
        );
        for i in 0..100 {
            let name = if i == 0 {
                format!("{base}{ext}")
            } else {
                format!("{base} {i}{ext}")
            };
            let p = golike::join(&[&dir, &name]);
            match self.create_excl(&p, data) {
                Err(Error::Exists) => continue,
                Err(e) => return Err(e),
                Ok(()) => return Ok(p),
            }
        }
        Err(Error::Exists)
    }

    /// Renames or moves a file or folder inside the vault, creating the destination folders.
    /// `Exists` if the destination is there (never overwrites). It only runs on the writer thread.
    fn move_locked(&self, from: &str, to: &str) -> Result<String> {
        let from = clean(from)?;
        let to = clean(to)?;
        if from == to {
            return Ok(to);
        }
        if !valid_name(&to) {
            return Err(Error::BadName);
        }
        self.root
            .symlink_metadata(&from)
            .map_err(|e| Error::io("statat", &from, e))?;
        // Moving a folder into itself (a -> a/b) would make everything unreachable.
        if to.starts_with(&format!("{from}/")) {
            return Err(Error::InvalidPath);
        }
        // Known limitation, same as the Go version: between this check and the rename another
        // program could create the destination (no RENAME_NOREPLACE through the capability API).
        match self.root.symlink_metadata(&to) {
            Ok(_) => return Err(Error::Exists),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io("statat", &to, e)),
        }
        let dir = golike::dir(&to);
        if dir != "." {
            self.mkdir_all(&dir)?;
        }
        self.rename_noreplace(&from, &to)?;
        Ok(to)
    }

    /// Moves a file or folder to the vault trash, deleting nothing. If the trash already has
    /// something with that name, date and time are appended. It only runs on the writer thread.
    fn trash_locked(&self, p: &str) -> Result<String> {
        let p = clean(p)?;
        self.root
            .symlink_metadata(&p)
            .map_err(|e| Error::io("statat", &p, e))?;
        self.mkdir_all(TRASH_DIR)?;
        let name = golike::base(&p).to_string();
        let mut dest = golike::join(&[TRASH_DIR, &name]);
        let stamp = chrono::Local::now()
            .format("%Y-%m-%d %H%M%S%.3f")
            .to_string();
        let ext = golike::ext(&name).to_string();
        let stem = name.strip_suffix(&ext).unwrap_or(&name).to_string();
        let mut i = 0usize;
        loop {
            if i > 0 {
                let suffix = if i > 1 {
                    format!("{stamp} {}", i - 1)
                } else {
                    stamp.clone()
                };
                dest = golike::join(&[TRASH_DIR, &format!("{stem} {suffix}{ext}")]);
            }
            match self.rename_noreplace(&p, &dest) {
                Ok(()) => break,
                Err(Error::Exists) => i += 1,
                Err(e) => return Err(e),
            }
        }
        Ok(dest)
    }

    /// Opens a vault file for reading (PDF, EPUB, images).
    pub fn open_file_sync(&self, p: &str) -> Result<File> {
        let p = clean(p)?;
        self.root.open(&p).map_err(|e| Error::io("openat", &p, e))
    }
}

impl Vault {
    // ---- async API -----------------------------------------------------------------

    /// Runs `f` on Tokio's blocking pool (independent operations).
    async fn blocking<T, F>(self: &Arc<Self>, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Core) -> Result<T> + Send + 'static,
    {
        let core = Arc::clone(&self.core);
        tokio::task::spawn_blocking(move || f(&core))
            .await
            .map_err(|e| Error::Internal(format!("vault task failed: {e}")))?
    }

    /// Runs `f` on the writer thread: operations queued here never overlap.
    async fn serialised<T, F>(self: &Arc<Self>, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Core) -> Result<T> + Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.writer
            .send(Box::new(move |c: &Core| {
                let _ = tx.send(f(c));
            }))
            .map_err(|_| Error::Internal("vault writer stopped".into()))?;
        rx.await
            .map_err(|_| Error::Internal("vault writer failed".into()))?
    }

    pub async fn tree(self: &Arc<Self>) -> Result<Vec<Entry>> {
        self.blocking(|v| v.tree_sync()).await
    }

    pub async fn read(self: &Arc<Self>, p: &str) -> Result<Note> {
        let p = p.to_string();
        self.blocking(move |v| v.read_sync(&p)).await
    }

    /// Reads many notes in one blocking task; the ones that are gone or unreadable are skipped.
    pub async fn read_many(self: &Arc<Self>, paths: Vec<String>) -> Result<Vec<Note>> {
        self.blocking(move |v| Ok(paths.iter().filter_map(|p| v.read_sync(p).ok()).collect()))
            .await
    }

    pub async fn exists(self: &Arc<Self>, p: &str) -> bool {
        let p = p.to_string();
        self.blocking(move |v| Ok(v.exists_sync(&p)))
            .await
            .unwrap_or(false)
    }

    /// Atomic save; `Err(Conflict(current))` when `base_mtime` is stale.
    pub async fn save(self: &Arc<Self>, p: &str, content: &str, base_mtime: i64) -> Result<Note> {
        let (p, content) = (p.to_string(), content.to_string());
        self.serialised(move |v| v.save_locked(&p, &content, base_mtime))
            .await
    }

    pub async fn mkdir(self: &Arc<Self>, p: &str) -> Result<String> {
        let p = p.to_string();
        self.blocking(move |v| v.mkdir_sync(&p)).await
    }

    pub async fn create(self: &Arc<Self>, p: &str, content: &str) -> Result<Note> {
        let (p, content) = (p.to_string(), content.to_string());
        self.blocking(move |v| v.create_sync(&p, &content)).await
    }

    pub async fn save_image(self: &Arc<Self>, note: &str, data: Vec<u8>) -> Result<String> {
        let note = note.to_string();
        self.blocking(move |v| v.save_image_sync(&note, &data))
            .await
    }

    pub async fn move_to(self: &Arc<Self>, from: &str, to: &str) -> Result<String> {
        let (from, to) = (from.to_string(), to.to_string());
        self.serialised(move |v| v.move_locked(&from, &to)).await
    }

    pub async fn trash(self: &Arc<Self>, p: &str) -> Result<String> {
        let p = p.to_string();
        self.serialised(move |v| v.trash_locked(&p)).await
    }

    pub async fn open_file(self: &Arc<Self>, p: &str) -> Result<File> {
        let p = p.to_string();
        self.blocking(move |v| v.open_file_sync(&p)).await
    }
}

/// Extension by content (not by the uploaded name). SVG is excluded: it may contain scripts.
/// Same signatures as Go's `http.DetectContentType` for these formats.
fn sniff_image_ext(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(".png")
    } else if data.starts_with(b"\xFF\xD8\xFF") {
        Some(".jpg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some(".gif")
    } else if data.len() >= 14 && data.starts_with(b"RIFF") && &data[8..14] == b"WEBPVP" {
        Some(".webp")
    } else if data.starts_with(b"BM") {
        Some(".bmp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
