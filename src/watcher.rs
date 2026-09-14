//! Filesystem watcher that nudges the code index on changes.

use std::path::PathBuf;
use std::sync::Arc;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::scan::scan_project;

/// Snapshot pushed to the watcher callback.
#[derive(Debug, Clone)]
pub struct IndexStatus {
    pub state: String,
    pub files: usize,
}

/// Watch `root` recursively; on create/modify/remove events rescan (capped)
/// and invoke `callback`. No debounce, no tokio.
pub fn spawn_watcher(
    root: PathBuf,
    callback: impl Fn(IndexStatus) + Send + Sync + 'static,
) -> Result<RecommendedWatcher, notify::Error> {
    let callback = Arc::new(callback);
    let scan_root = root.clone();
    let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            let relevant = match res {
                Ok(ev) => matches!(
                    ev.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ),
                Err(_) => false,
            };
            if !relevant {
                return;
            }
            let files = scan_project(&scan_root, &[], 20_000).len();
            callback(IndexStatus {
                state: "indexing".to_string(),
                files,
            });
        })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;
    Ok(watcher)
}
