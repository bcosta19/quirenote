//! Watches the vault and forwards changes to the connected browsers (SSE in `api`).
//! Port of internal/watch.
//!
//! Directories are watched one by one (not recursively), skipping hidden ones, exactly as the
//! Go version does with fsnotify, so `.obsidian`, `.trash` and `.git` never cost inotify watches.
//! Repeated events for the same file are coalesced (150 ms debounce) since editors often
//! produce several writes per save.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecursiveMode, Watcher};
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

/// Handle that stops the watcher thread when dropped.
pub struct Handle {
    stop: Arc<AtomicBool>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn op_name(kind: &EventKind) -> Option<&'static str> {
    match kind {
        EventKind::Remove(_) => Some("remove"),
        EventKind::Create(_) => Some("create"),
        // fsnotify: IN_MOVED_FROM is Rename, IN_MOVED_TO is Create.
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => Some("rename"),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => Some("create"),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => None,
        EventKind::Modify(ModifyKind::Name(_)) => Some("rename"),
        EventKind::Modify(_) => Some("write"),
        _ => None,
    }
}

fn add_tree(w: &mut impl Watcher, root: &Path, top: &Path) {
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if dir != top
            && dir
                .file_name()
                .is_some_and(|n| hidden(&n.to_string_lossy()))
        {
            continue;
        }
        if let Err(e) = w.watch(&dir, RecursiveMode::NonRecursive) {
            eprintln!("watch: add dir={} err={e}", dir.display());
            continue;
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
}

/// Starts watching `dir` on its own thread; the returned handle stops it when dropped.
pub fn spawn(dir: PathBuf, broker: Broker) -> std::io::Result<Handle> {
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .map_err(std::io::Error::other)?;
    add_tree(&mut watcher, &dir, &dir);

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    std::thread::Builder::new()
        .name("watch".into())
        .spawn(move || event_loop(watcher, rx, dir, broker, stop2))?;
    Ok(Handle { stop })
}

fn event_loop(
    mut watcher: impl Watcher,
    rx: mpsc::Receiver<notify::Result<notify::Event>>,
    dir: PathBuf,
    broker: Broker,
    stop: Arc<AtomicBool>,
) {
    let mut pending: HashMap<String, Event> = HashMap::new();
    let mut deadline: Option<Instant> = None;
    while !stop.load(Ordering::SeqCst) {
        let wait = match deadline {
            Some(d) => d.saturating_duration_since(Instant::now()),
            None => Duration::from_millis(500),
        };
        match rx.recv_timeout(wait) {
            Ok(Ok(ev)) => {
                let Some(op) = op_name(&ev.kind) else {
                    continue;
                };
                for path in &ev.paths {
                    let Ok(rel) = path.strip_prefix(&dir) else {
                        continue;
                    };
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if rel.is_empty() || hidden(&rel) {
                        continue;
                    }
                    if op == "create" && path.is_dir() {
                        add_tree(&mut watcher, path, &dir);
                    }
                    pending.insert(
                        rel.clone(),
                        Event {
                            path: rel,
                            op: op.to_string(),
                        },
                    );
                    deadline = Some(Instant::now() + DEBOUNCE);
                }
            }
            Ok(Err(e)) => eprintln!("watch: err={e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    for (_, e) in pending.drain() {
                        broker.publish(e);
                    }
                    deadline = None;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
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
        std::thread::sleep(Duration::from_millis(200));

        std::fs::write(root.join(".obsidian/x.json"), "{}").unwrap();
        std::fs::write(root.join("a.md"), "hello").unwrap();
        let ev = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev.path, "a.md");

        // New folders are watched too.
        std::fs::create_dir(root.join("sub")).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        while rx.try_recv().is_ok() {}
        std::fs::write(root.join("sub/b.md"), "x").unwrap();
        let ev = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev.path, "sub/b.md");
    }
}
