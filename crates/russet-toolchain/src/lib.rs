// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Toolchains (ARCHITECTURE §16.1–§16.2; core §5.6): the pin manifest, probes, the resolver with its env
//! contract (`ToolInfo.env` is part of the contract), the installer, GC and the SDK registry probe.
//!
//! F0 stub (W1); W3 fills the bodies (CORE-10, CORE-11 subset). Layer L2 may not depend on russet-doctor
//! (ARCHITECTURE §5.2), so toolchain checks register from russetd (`handlers/doctor.rs`).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use russet_core::Result;
use russet_core::progress::ProgressSink;
use tokio_util::sync::CancellationToken;

/// A toolchain component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Component {
    Swift,
    DarwinSdk,
    Linker,
    Lldb,
    LldbDap,
    LibPython,
    Xtool,
    Flutter,
    GenSnapshotIos,
    Llvm,
}

/// Where a resolved tool came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    Managed {
        pin: String,
    },
    System,
    /// An existing uv-managed python-build-standalone (ARCHITECTURE §16.1).
    Uv,
    XtoolBundle {
        bundle: PathBuf,
    },
    User,
    ProjectPin,
}

/// A resolved tool. `env` is part of the contract: every launcher applies it (ARCHITECTURE §16.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInfo {
    pub component: Component,
    pub version: String,
    pub path: PathBuf,
    pub provenance: Provenance,
    pub env: BTreeMap<String, String>,
    pub ok: bool,
    pub problems: Vec<String>,
}

/// A Darwin SDK known to the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkInfo {
    /// For example `xcode-27.0-27A266a`.
    pub id: String,
    pub xcode_version: String,
    pub xcode_build: String,
    pub ios_sdk: String,
    pub swift_sdk_name: String,
    pub bundle: PathBuf,
    pub darwin_tools: Option<String>,
    pub compatible_swift: String,
}

/// A version requirement for [`Toolchains::resolve`]. W3 defines the fields (project pin, minimum, flavor).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Requirement {
    pub version: Option<String>,
}

/// What [`Toolchains::gc`] removed (or would remove).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    pub freed_bytes: u64,
    pub removed: Vec<PathBuf>,
}

/// The toolchain manager.
#[derive(Debug)]
pub struct Toolchains {
    _f0: (),
}

impl Toolchains {
    pub async fn probe_all(&self, refresh: bool) -> Vec<ToolInfo> {
        todo!("W3 (CORE-10)")
    }
    pub async fn sdks(&self) -> Result<Vec<SdkInfo>> {
        todo!("W3 (CORE-10)")
    }
    pub async fn resolve(&self, c: Component, req: &Requirement) -> Result<ToolInfo> {
        todo!("W3 (CORE-10)")
    }
    pub async fn install(
        &self,
        c: Component,
        version: &str,
        p: &dyn ProgressSink,
        cancel: &CancellationToken,
    ) -> Result<ToolInfo> {
        todo!("W3 (CORE-11)")
    }
    /// Returns the bytes freed.
    pub async fn uninstall(&self, c: Component, version: &str) -> Result<u64> {
        todo!("W3 (CORE-11)")
    }
    pub async fn gc(&self, dry_run: bool) -> Result<GcReport> {
        todo!("W3 (CORE-11)")
    }
}
