//! Prepares the vault on first run: a new empty folder or an existing folder of markdown
//! notes; an Obsidian vault works as is, nothing is copied (port of internal/setup).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::go_quote;
use crate::golike;

/// The folder suggested for a fresh start.
pub const DEFAULT_PATH: &str = "~/Quirenote";

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Prepared {
    pub dir: String,
    /// The folder is an Obsidian vault (`.obsidian/` exists).
    pub obsidian: bool,
}

/// Expands "~", requires an absolute path and refuses folders that would expose too much:
/// the filesystem root, the home folder itself and its parents.
pub fn resolve(path: &str, home: &str) -> Result<String, String> {
    let path = path.trim();
    let expanded;
    let path = if path == "~" || path.starts_with("~/") {
        expanded = golike::join(&[home, path.strip_prefix('~').unwrap_or(path)]);
        expanded.as_str()
    } else {
        path
    };
    if !path.starts_with('/') {
        return Err("use an absolute path (or one starting with ~/)".into());
    }
    let dir = canonical(Path::new(&golike::clean(path))).map_err(|e| e.to_string())?;
    let home = canonical(Path::new(&golike::clean(home))).map_err(|e| e.to_string())?;
    if dir == "/" || dir == home || home.starts_with(&format!("{dir}/")) {
        return Err(format!(
            "{dir} is too broad for a vault; pick a folder inside it"
        ));
    }
    Ok(dir)
}

/// Resolves symlinks of the part of the path that exists, keeping the rest as is.
fn canonical(dir: &Path) -> io::Result<String> {
    match fs::canonicalize(dir) {
        Ok(p) => Ok(p.to_string_lossy().into_owned()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
                return Err(e);
            };
            let resolved = canonical(parent)?;
            Ok(PathBuf::from(resolved)
                .join(name)
                .to_string_lossy()
                .into_owned())
        }
        Err(e) => Err(e),
    }
}

/// Validates (and for "new", creates) the vault folder. It does not save the config.
pub fn prepare(mode: &str, path: &str, home: &str) -> Result<Prepared, String> {
    let dir = resolve(path, home)?;
    match mode {
        "new" => {
            create(&dir)?;
            Ok(Prepared {
                dir,
                obsidian: false,
            })
        }
        "existing" => {
            let info = fs::metadata(&dir).map_err(|_| format!("folder not found: {dir}"))?;
            if !info.is_dir() {
                return Err(format!("not a folder: {dir}"));
            }
            writable(&dir)?;
            let obsidian =
                fs::metadata(Path::new(&dir).join(".obsidian")).is_ok_and(|m| m.is_dir());
            Ok(Prepared { dir, obsidian })
        }
        other => Err(format!("unknown mode {}", go_quote(other))),
    }
}

/// Picks the mode for the CLI: an existing folder is used as is, a missing one is created.
pub fn auto(path: &str, home: &str) -> Result<Prepared, String> {
    let dir = resolve(path, home)?;
    if fs::metadata(&dir).is_ok() {
        prepare("existing", &dir, home)
    } else {
        prepare("new", &dir, home)
    }
}

/// Makes the folder (it may exist, but must be empty) and adds the welcome note.
fn create(dir: &str) -> Result<(), String> {
    if let Ok(mut items) = fs::read_dir(dir)
        && items.next().is_some()
    {
        return Err(format!(
            "{dir} is not empty; to keep its notes, choose \"use an existing folder\""
        ));
    }
    fs::DirBuilder::new()
        .recursive(true)
        .create(dir)
        .map_err(|e| e.to_string())?;
    fs::write(Path::new(dir).join("Welcome.md"), WELCOME).map_err(|e| e.to_string())
}

fn writable(dir: &str) -> Result<(), String> {
    let mut b = [0u8; 4];
    getrandom::fill(&mut b).map_err(|e| e.to_string())?;
    let name = format!(".quirenote-check-{}", u32::from_le_bytes(b));
    let p = Path::new(dir).join(name);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map_err(|e| format!("cannot write to {dir}: {}", crate::vault::go_errno(&e)))?;
    fs::remove_file(&p).map_err(|e| e.to_string())
}

const WELCOME: &str = "# Welcome to Quirenote

Your notes are plain markdown files in this folder: you can also edit them in Neovim,
Obsidian or any text editor, and Quirenote picks up the changes.

## Getting started

- Create a note with the **new note** button in the sidebar, or press `Ctrl+O`,
  type a name and press `Shift+Enter`.
- Link notes with `[[Note name]]`; `Ctrl+click` follows a link.
- Open a PDF or EPUB from the sidebar to read it next to your notes, and press
  `Ctrl+Shift+L` to insert a link to the current page.
- `Ctrl+E` toggles the preview; `Ctrl+Shift+F` searches all notes.

- [ ] Create your first note
- [ ] Open a PDF and link to a page
";

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn resolve_cases() {
        let home = "/home/u";
        for (i, want) in [
            ("~/Notes", "/home/u/Notes"),
            ("/data/notes/", "/data/notes"),
            (" ~/a/../b ", "/home/u/b"),
        ] {
            assert_eq!(resolve(i, home).unwrap(), want, "{i}");
        }
        for bad in ["/", "~", "/home", "/home/u/", "notes", "../x", ""] {
            assert!(resolve(bad, home).is_err(), "resolve({bad:?}) should fail");
        }
    }

    #[test]
    fn resolve_rejects_symlink_aliases_of_broad_folders() {
        let base = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(base.path()).unwrap();
        let home = base.join("home");
        fs::create_dir(&home).unwrap();
        let home_s = home.to_str().unwrap();
        for (name, target) in [
            ("home-link", home.clone()),
            ("parent-link", base.clone()),
            ("root-link", PathBuf::from("/")),
        ] {
            let alias = base.join(name);
            symlink(&target, &alias).unwrap();
            assert!(
                resolve(alias.to_str().unwrap(), home_s).is_err(),
                "{name} must be rejected"
            );
        }
        let alias = base.join("home-link");
        assert!(resolve(home_s, alias.to_str().unwrap()).is_err());
        let want = home.join("new").join("notes");
        assert_eq!(
            resolve(alias.join("new").join("notes").to_str().unwrap(), home_s).unwrap(),
            want.to_str().unwrap()
        );
    }

    #[test]
    fn prepare_modes() {
        let h = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(h.path()).unwrap();
        let home_s = home.to_str().unwrap();
        let r = prepare("new", "~/Quirenote", home_s).unwrap();
        assert_eq!(r.dir, home.join("Quirenote").to_str().unwrap());
        assert!(!r.obsidian);
        assert!(home.join("Quirenote/Welcome.md").exists());
        assert!(prepare("new", &r.dir, home_s).is_err());

        let vault = home.join("Vault");
        fs::create_dir_all(vault.join(".obsidian")).unwrap();
        let r = prepare("existing", vault.to_str().unwrap(), home_s).unwrap();
        assert!(r.obsidian);
        assert_eq!(fs::read_dir(&vault).unwrap().count(), 1);
        assert!(prepare("existing", home.join("missing").to_str().unwrap(), home_s).is_err());
        assert!(auto(vault.to_str().unwrap(), home_s).unwrap().obsidian);
        assert!(!auto("~/Fresh", home_s).unwrap().obsidian);
        assert!(prepare("other", "~/x", home_s).is_err());
    }
}
