// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Russet's five directories (ARCHITECTURE §15): config, data, state, cache and runtime. With `RUSSET_HOME`
//! set, all five move under it (ARCHITECTURE §3.3 item 7, §14.9).

use std::path::{Path, PathBuf};

/// The resolved directory set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RussetDirs {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
    pub runtime: PathBuf,
}

impl RussetDirs {
    /// Resolves from `RUSSET_HOME`, `RUSSET_{CONFIG,DATA,STATE,CACHE,RUNTIME}_DIR`, the XDG variables, or
    /// `%APPDATA%`/`%LOCALAPPDATA%` on Windows.
    pub fn from_env() -> crate::Result<Self> {
        todo!("W2 (CORE-02): ARCHITECTURE §15 resolution rules")
    }

    /// Portable layout: every directory under `root` (what `RUSSET_HOME=<root>` selects).
    pub fn under(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            state: root.join("state"),
            cache: root.join("cache"),
            runtime: root.join("runtime"),
        }
    }

    /// Creates the directories 0700 and checks the owner (Unix), or sets a user-only ACL (Windows).
    pub fn ensure(&self) -> crate::Result<()> {
        todo!("W2 (CORE-02): create 0700 and check ownership")
    }
}
