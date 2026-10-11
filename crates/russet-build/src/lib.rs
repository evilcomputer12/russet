// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The build orchestrator (ARCHITECTURE §5.3 `russet-build`; build §9.1): project detection, stack adapters
//! (SwiftPM now; Flutter, Xcode, React Native, MAUI later), Info.plist synthesis, bundle assembly, lints and the
//! metadata policy. Events are `JobEvent` kinds through `russet_core::progress::EventSink`.
//!
//! F0 stub (W1); W5 fills it (Wave 1), W16 (Wave 2); the stack, resource and platform modules belong to the
//! cards listed in PLAN §3.

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_core::ids::JobId;
use russet_core::progress::EventSink;
use russet_sdk::SdkProvider;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub mod bundle;
pub mod infoplist;
pub mod metadata;
pub mod platform;
pub mod resources;
pub mod stack;

/// What a [`Builder`] needs from its environment (build §9.1).
#[derive(Clone)]
pub struct BuildEnv {
    pub sdk: Arc<dyn SdkProvider>,
    /// `russet-tool` multi-call links (`xcrun`, `actool`, …).
    pub tools_dir: Utf8PathBuf,
    pub data_dir: Utf8PathBuf,
    pub cache_dir: Utf8PathBuf,
}

/// A detected project. W5 defines the fields (`project.detect`, `project.info`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub root: Utf8PathBuf,
}

/// A build request (`build.start` params). W5 defines the remaining fields (build §9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    pub project: Utf8PathBuf,
    pub target: Option<String>,
    pub configuration: Option<String>,
    pub bundle_id_policy: BundleIdPolicy,
    pub overrides: BTreeMap<String, String>,
    pub clean: bool,
}

/// A dry-run plan (`build.plan`). W5 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    _f0: (),
}

/// What `build.clean` removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanScope {
    Build,
    All,
}

/// What `build.clean` freed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanReport {
    pub freed_bytes: u64,
}

/// How effective bundle IDs are derived (ARCHITECTURE §16.5) [D 18].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleIdPolicy {
    Exact,
    TeamSuffix { team_id: String },
    XtoolPrefix { prefix: String },
    Custom(BTreeMap<String, String>),
}

/// A recorded consent for `xcode-compat` metadata (`[build] metadata-consent = {by, at}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consent {
    pub by: String,
    pub at: String,
}

/// `Info.plist` DT* key policy (ARCHITECTURE §16.4) [D 17].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataPolicy {
    None,
    Minimal,
    Sdk,
    XcodeCompat { consent: Consent },
}

/// One built bundle: path, executable, effective bundle id, versions. W5 defines the remaining fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleInfo {
    pub path: Utf8PathBuf,
    pub bundle_id: String,
}

/// What a nested item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignKind {
    Bundle,
    MachO,
}

/// One item to sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignItem {
    pub path: Utf8PathBuf,
    pub kind: SignKind,
    pub entitlements: Option<Utf8PathBuf>,
    pub get_task_allow: bool,
}

/// Items to sign, inside-out: deepest first, app last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SigningPlan {
    pub items: Vec<SignItem>,
}

/// A build lint finding. W5 and W16 define the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintFinding {
    pub code: String,
    pub message: String,
}

/// The result of `build.start` (ARCHITECTURE §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutput {
    pub build_id: JobId,
    pub app: BundleInfo,
    pub extensions: Vec<BundleInfo>,
    /// Original → effective bundle IDs.
    pub bundle_ids: BTreeMap<String, String>,
    pub signing_plan: SigningPlan,
    pub dsyms: Vec<Utf8PathBuf>,
    pub ipa: Option<Utf8PathBuf>,
    pub report_path: Utf8PathBuf,
    pub lints: Vec<LintFinding>,
    pub duration: Duration,
}

/// What russet-debug consumes from a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltApp {
    pub app: Utf8PathBuf,
    pub executable: Utf8PathBuf,
    pub uuids: Vec<Uuid>,
    pub dsyms: Vec<Utf8PathBuf>,
    pub sdk_path: Utf8PathBuf,
    pub swift_module_dirs: Vec<Utf8PathBuf>,
    pub flutter_framework: Option<Utf8PathBuf>,
}

/// The build orchestrator.
pub struct Builder {
    _f0: (),
}

impl Builder {
    pub fn new(env: BuildEnv) -> Result<Self> {
        todo!("W5 (B02)")
    }
    pub async fn detect(&self, dir: &Utf8Path) -> Result<ProjectInfo> {
        todo!("W5 (B02)")
    }
    pub async fn plan(&self, r: &BuildRequest) -> Result<BuildPlan> {
        todo!("W5 (B02)")
    }
    pub async fn build(&self, r: BuildRequest, sink: &dyn EventSink, cancel: CancellationToken) -> Result<BuildOutput> {
        todo!("W5 (B03)")
    }
    pub async fn resolve_info_plist(&self, r: &BuildRequest, product: Option<&str>) -> Result<plist::Dictionary> {
        todo!("W5 (B06)")
    }
    pub async fn clean(&self, dir: &Utf8Path, scope: CleanScope) -> Result<CleanReport> {
        todo!("W5 (B02)")
    }
}

/// Build doctor checks, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
