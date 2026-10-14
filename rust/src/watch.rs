//! Watches the vault and forwards changes to the connected browsers (SSE in `api`).
//! Port of internal/watch.
//!
//! Plain inotify (the `inotify` crate) with exactly the mask fsnotify uses in Go: create, delete,
//! modify, attrib, move. The more convenient `notify` crate also subscribes to `IN_OPEN`, so every
//! file the server (or `rg`) merely *opens* produced an event: a search over 10,000 notes meant
//! 30,000 useless events to decode, queue and drop, and its memory churn dominated the process.
//!
//! Directories are watched one by one (not recursively), skipping hidden ones, as the Go version
//! does, so `.obsidian`, `.trash` and `.git` never cost an inotify watch. Repeated events for the
//! same file are coalesced (150 ms debounce) since editors often produce several writes per save.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};
use serde::Serialize;
use tokio::sync::broadcast;

use crate::vault::hidden;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Event {
    /// Relative to the vault.
    pub path: String,
    /// write | create | remove | rename
    pub op: String,
}

/// Distributes events to the subscribers. One shared ring buffer (`broadcast`) instead of a
/// channel per client: a subscriber costs a few bytes, and a slow one just loses events.
#[derive(Clone)]
pub struct Broker {
    tx: broadcast::Sender<Event>,
}

impl Default for Broker {
    fn default() -> Self {
        Self::new()
    }
}

impl Broker {
    pub fn new() -> Broker {
        // Same depth as the per-subscriber channel of the Go version (32).
        let (tx, _) = broadcast::channel(32);
        Broker { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn publish(&self, e: Event) {
        // No subscribers is not an error.
        let _ = self.tx.send(e);
    }
}

const DEBOUNCE: Duration = Duration::from_millis(150);

/// fsnotify's inotify mask (internal/backend_inotify.go): no OPEN/ACCESS/CLOSE events.
fn mask() -> WatchMask {
    WatchMask::MOVED_TO
        | WatchMask::MOVED_FROM
        | WatchMask::CREATE
        | WatchMask::ATTRIB
        | WatchMask::MODIFY
        | WatchMask::MOVE_SELF
        | WatchMask::DELETE
        | WatchMask::DELETE_SELF
}

/// Stops the watcher task when dropped.
pub struct Handle {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// fsnotify's op names: Remove wins over Rename, then Create, anything else is a write.
fn op_name(m: EventMask) -> Option<&'static str> {
    if m.intersects(EventMask::DELETE | EventMask::DELETE_SELF) {
        Some("remove")
    } else if m.intersects(EventMask::MOVED_FROM | EventMask::MOVE_SELF) {
        Some("rename")
    } else if m.intersects(EventMask::CREATE | EventMask::MOVED_TO) {
        Some("create")
    } else if m.intersects(EventMask::MODIFY | EventMask::ATTRIB) {
        Some("write")
    } else {
        None
    }
}

/// Watches `root` and every non-hidden directory below it (symlinks are not followed).
fn add_tree(watches: &mut Watches, root: &Path, top: &Path) -> Vec<(WatchDescriptor, PathBuf)> {
    let mut added = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if dir != top
            && dir
                .file_name()
                .is_some_and(|n| hidden(&n.to_string_lossy()))
        {
            continue;
        }
        match watches.add(&dir, mask()) {
            Ok(wd) => added.push((wd, dir.clone())),
            Err(e) => {
                eprintln!("watch: add dir={} err={e}", dir.display());
                continue;
            }
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for it in rd.flatten() {
            if it.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(it.path());
            }
        }
    }
    added
}

/// Starts watching `dir` on a Tokio task (so it must be called inside a runtime); the returned
/// handle stops it when dropped.
pub fn spawn(dir: PathBuf, broker: Broker) -> std::io::Result<Handle> {
    let inotify = Inotify::init()?;
    let mut watches = inotify.watches();
    let stream = inotify.into_event_stream(vec![0u8; 16 * 1024])?;
    let task = tokio::spawn(async move {
        // The directories are registered in the background (like Go's goroutine), so a large
        // vault does not delay the moment the server starts listening.
        let (mut w2, d2) = (watches.clone(), dir.clone());
        let added = tokio::task::spawn_blocking(move || add_tree(&mut w2, &d2, &d2))
            .await
            .unwrap_or_default();
        let mut dirs: HashMap<WatchDescriptor, PathBuf> = added.into_iter().collect();
        event_loop(stream, &mut watches, &mut dirs, &dir, &broker).await;
    });
    Ok(Handle { task })
}

async fn event_loop(
    mut stream: inotify::EventStream<Vec<u8>>,
    watches: &mut Watches,
    dirs: &mut HashMap<WatchDescriptor, PathBuf>,
    top: &Path,
    broker: &Broker,
) {
    let mut pending: HashMap<String, Event> = HashMap::new();
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        let timer = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            ev = stream.next() => {
                let ev = match ev {
                    Some(Ok(ev)) => ev,
                    Some(Err(e)) => {
                        eprintln!("watch: err={e}");
                        continue;
                    }
                    None => return,
                };
                if ev.mask.contains(EventMask::Q_OVERFLOW) {
                    eprintln!("watch: err=inotify event queue overflowed");
                    continue;
                }
                if ev.mask.contains(EventMask::IGNORED) {
                    dirs.remove(&ev.wd);
                    continue;
                }
                let (Some(op), Some(base)) = (op_name(ev.mask), dirs.get(&ev.wd)) else {
                    continue;
                };
                let full = match &ev.name {
                    Some(n) => base.join(n),
                    None => base.clone(),
                };
                let Ok(rel) = full.strip_prefix(top) else {
                    continue;
                };
                let rel = rel.to_string_lossy().replace('\\', "/");
                if rel.is_empty() || hidden(&rel) {
                    continue;
                }
                // A new (or moved-in) folder: watch it too. IN_ISDIR does not follow symlinks.
                if ev.mask.contains(EventMask::ISDIR)
                    && ev.mask.intersects(EventMask::CREATE | EventMask::MOVED_TO)
                {
                    for (wd, p) in add_tree(watches, &full, top) {
                        dirs.insert(wd, p);
                    }
                }
                pending.insert(rel.clone(), Event { path: rel, op: op.to_string() });
                deadline = Some(tokio::time::Instant::now() + DEBOUNCE);
            }
            () = timer => {
                for (_, e) in pending.drain() {
                    broker.publish(e);
                }
                deadline = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reports_external_changes_and_ignores_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join(".obsidian")).unwrap();
        let broker = Broker::new();
        let mut rx = broker.subscribe();
        let _h = spawn(root.clone(), broker.clone()).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        std::fs::write(root.join(".obsidian/x.json"), "{}").unwrap();
        std::fs::write(root.join("a.md"), "hello").unwrap();
        let ev = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev.path, "a.md");

        // New folders are watched too.
        std::fs::create_dir(root.join("sub")).unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        while rx.try_recv().is_ok() {}
        std::fs::write(root.join("sub/b.md"), "x").unwrap();
        let ev = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev.path, "sub/b.md");
    }
}
