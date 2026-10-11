// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Typed configuration, layering, the comment-preserving editor, JSON Schemas and the watcher (ARCHITECTURE
//! §14; core §5.4). [`ConfigService`] is the daemon-side service that `DaemonCtx.config` names [D 86].
//!
//! F0 stub (W1); W3 fills the bodies (CORE-08). Layer L2 may not depend on russet-doctor (ARCHITECTURE §5.2),
//! so configuration checks register from russetd (`handlers/doctor.rs`).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russet_core::Result;
use russet_core::dirs::RussetDirs;
use tokio::sync::broadcast;

/// The global `config.toml` (ARCHITECTURE §14.3). W3 defines the fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalConfig {}

/// A project's `russet.toml` plus `russet.local.toml` (ARCHITECTURE §14.2). W3 defines the fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectConfig {}

/// The merged view after the precedence of ARCHITECTURE §14.1 (`[agents]` only narrows).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effective {}

/// Which file an edit goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigScope {
    Global,
    Project,
    Local,
}

/// A comment- and order-preserving editor over one TOML file (toml_edit).
#[derive(Debug)]
pub struct ConfigEditor {
    _f0: (),
}

/// A configuration file changed (`config.changed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigChanged {
    pub scope: ConfigScope,
    pub path: PathBuf,
}

/// The global configuration, a per-project cache and the watcher.
#[derive(Debug)]
pub struct ConfigService {
    _f0: (),
}

impl ConfigService {
    pub fn load(dirs: &RussetDirs) -> Result<Self> {
        todo!("W3 (CORE-08)")
    }
    pub fn global(&self) -> Arc<GlobalConfig> {
        todo!("W3 (CORE-08)")
    }
    /// ARCHITECTURE §14.1 precedence; a project's `[agents]` section only narrows.
    pub fn effective(&self, project: Option<&Path>) -> Result<Effective> {
        todo!("W3 (CORE-08)")
    }
    pub fn editor(&self, scope: ConfigScope, project: Option<&Path>) -> Result<ConfigEditor> {
        todo!("W3 (CORE-08)")
    }
    pub fn subscribe(&self) -> broadcast::Receiver<ConfigChanged> {
        todo!("W3 (CORE-08)")
    }
}
