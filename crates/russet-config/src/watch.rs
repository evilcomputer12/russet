// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The file watcher behind `config.changed`: it watches the config directory and every project root the service
//! has loaded (non-recursively) and maps events on Russet's files to [`ConfigChanged`].
//!
//! Russet's own edits are reported twice (by the editor after it saves, and by the watcher that sees the write),
//! and editors write in bursts, so both go through one [`ChangeSender`], which reports each state of a file once:
//! a change whose file stamp (modification time and length) was already reported is dropped.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::broadcast;

use crate::load::{
    ACCOUNTS_FILE, GLOBAL_FILE, LOCAL_FILE, MCP_POLICY_FILE, PROJECT_FILE, PROVISION_POLICY_FILE, Stamp,
};
use crate::{ConfigChanged, ConfigScope};

/// Sends `config.changed` once per state of a file (see the module docs).
#[derive(Debug, Clone)]
pub struct ChangeSender {
    tx: broadcast::Sender<ConfigChanged>,
    reported: Arc<Mutex<HashMap<PathBuf, Option<Stamp>>>>,
}

impl ChangeSender {
    /// A sender over `tx`.
    pub fn new(tx: broadcast::Sender<ConfigChanged>) -> Self {
        Self { tx, reported: Arc::default() }
    }

    /// Reports `change`, unless the file is in a state that was already reported.
    pub fn send(&self, change: ConfigChanged) {
        let stamp = Stamp::of(&change.path);
        {
            let mut reported = self.reported.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if reported.get(&change.path) == Some(&stamp) {
                return;
            }
            reported.insert(change.path.clone(), stamp);
        }
        // No receiver is fine: nobody listens yet.
        let _ = self.tx.send(change);
    }
}

/// The scope of a changed file, or `None` when it is not a configuration file.
pub fn scope_of(path: &Path) -> Option<ConfigScope> {
    match path.file_name()?.to_str()? {
        GLOBAL_FILE | ACCOUNTS_FILE | MCP_POLICY_FILE | PROVISION_POLICY_FILE => Some(ConfigScope::Global),
        PROJECT_FILE => Some(ConfigScope::Project),
        LOCAL_FILE => Some(ConfigScope::Local),
        _ => None,
    }
}

/// A best-effort watcher: when the platform refuses (inotify limits, missing directories) the service still
/// works, and edits made through Russet still notify.
pub struct ConfigWatcher {
    watcher: Option<notify::RecommendedWatcher>,
    watched: BTreeSet<PathBuf>,
}

impl std::fmt::Debug for ConfigWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigWatcher")
            .field("active", &self.watcher.is_some())
            .field("watched", &self.watched)
            .finish()
    }
}

impl ConfigWatcher {
    /// Starts a watcher that reports through `changes`.
    pub fn start(changes: ChangeSender) -> Self {
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            for path in event.paths {
                let Some(scope) = scope_of(&path) else { continue };
                // A burst (write, close, rename) reports each state of the file once.
                changes.send(ConfigChanged { scope, path });
            }
        };
        Self { watcher: notify::recommended_watcher(handler).ok(), watched: BTreeSet::new() }
    }

    /// Watches `dir` (non-recursively) if it exists and is not watched yet.
    pub fn watch_dir(&mut self, dir: &Path) {
        if self.watched.contains(dir) || !dir.is_dir() {
            return;
        }
        if let Some(w) = self.watcher.as_mut()
            && w.watch(dir, RecursiveMode::NonRecursive).is_ok()
        {
            self.watched.insert(dir.to_path_buf());
        }
    }

    /// Whether the platform watcher is running.
    pub fn is_active(&self) -> bool {
        self.watcher.is_some()
    }
}
