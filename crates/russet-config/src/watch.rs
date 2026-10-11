// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The file watcher behind `config.changed`: it watches the config directory and every project root the service
//! has loaded (non-recursively) and maps events on Russet's files to [`ConfigChanged`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::broadcast;

use crate::load::{ACCOUNTS_FILE, GLOBAL_FILE, LOCAL_FILE, MCP_POLICY_FILE, PROJECT_FILE, PROVISION_POLICY_FILE};
use crate::{ConfigChanged, ConfigScope};

/// Events for the same file closer together than this are one change.
const BURST: Duration = Duration::from_millis(200);

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
    /// Starts a watcher that sends to `tx`.
    pub fn start(tx: broadcast::Sender<ConfigChanged>) -> Self {
        let last = Mutex::new(None::<(ConfigChanged, Instant)>);
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            for path in event.paths {
                let Some(scope) = scope_of(&path) else { continue };
                let change = ConfigChanged { scope, path };
                // Editors emit bursts (write, close, rename); report each file once per burst.
                let mut guard = match last.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if let Some((prev, at)) = guard.as_ref()
                    && *prev == change
                    && at.elapsed() < BURST
                {
                    continue;
                }
                *guard = Some((change.clone(), Instant::now()));
                let _ = tx.send(change);
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
