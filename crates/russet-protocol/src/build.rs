// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `build.*`: builds (ARCHITECTURE §8.3). Owner: W5 (Wave 1), W16 (build.settings).
//!
//! `build.start` is long-running: the request stays open until it returns [`BuildOutput`], or returns
//! `{jobId}` at once with `background: true` (ARCHITECTURE §7.6); progress, cancellation and events go through
//! `job.*`. `build.settings` keeps its F0 placeholder DTOs until W16 (Wave 2) defines them.

use std::collections::BTreeMap;

use russet_core::ids::JobId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::project::{ProjectStack, TargetKind};
use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// What to package after the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKind {
    None,
    /// A stored (uncompressed) IPA for fast development installs.
    IpaDev,
    /// A deflated IPA with `SwiftSupport/` for App Store uploads.
    IpaStore,
}

/// The SwiftPM build backend (`[build] backend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum BuildBackend {
    Swiftbuild,
    Native,
    Xtool,
    Pif,
}

/// How effective bundle IDs are derived (ARCHITECTURE §16.5) [D 18].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum BundleIdPolicy {
    /// `[app] bundle-id` exactly (paid teams).
    Exact,
    /// `<id>.<TEAMID>` (free teams).
    TeamSuffix { team_id: String },
    /// `XTL-<PREFIX>.<id>`, the prefix being the first segment of the xtool identity id.
    XtoolPrefix { prefix: String },
    /// An explicit original → effective map.
    Custom { map: BTreeMap<String, String> },
}

/// Params of `build.start` and `build.plan`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BuildStartParams {
    /// The absolute project root (or a directory inside it).
    pub project: String,
    /// The app product; `[build.swiftpm] product` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<String>,
    /// `debug`, `release`, `profile` or a named profile; `[build] default-profile` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub configuration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub package: Option<PackageKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub backend: Option<BuildBackend>,
    /// Build-setting overrides (`KEY = VALUE`), on top of `[build] overrides`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub overrides: Option<BTreeMap<String, String>>,
    /// Remove the SwiftPM scratch and the profile's outputs first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub clean: Option<bool>,
    /// The signing area's policy; `[signing] bundle-id-policy` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id_policy: Option<BundleIdPolicy>,
}

/// One built bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BundleInfo {
    pub path: String,
    pub executable: String,
    /// The effective `CFBundleIdentifier`.
    pub bundle_id: String,
    /// The canonical ID before the bundle-ID policy.
    pub original_bundle_id: String,
    pub kind: TargetKind,
    pub product: String,
    /// `CFBundleShortVersionString`.
    pub version: String,
    /// `CFBundleVersion`.
    pub build_number: String,
}

/// What a signing-plan item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SignKind {
    Bundle,
    MachO,
}

/// One item to sign.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignItem {
    pub path: String,
    pub kind: SignKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub entitlements: Option<String>,
    pub get_task_allow: bool,
}

/// Items to sign, inside-out: deepest first, the app last.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SigningPlan {
    pub items: Vec<SignItem>,
}

/// Lint severities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum LintSeverity {
    Info,
    Warning,
    Error,
}

/// A lint finding (build §4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LintFinding {
    /// `L001` … `L012`.
    pub id: String,
    pub severity: LintSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    pub message: String,
    /// Whether the build fixed it.
    pub fixed: bool,
}

/// Result of `build.start` (ARCHITECTURE §5.3 `BuildOutput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BuildOutput {
    /// The build's job id (`job_…`).
    pub build_id: JobId,
    pub app: BundleInfo,
    pub extensions: Vec<BundleInfo>,
    /// Original → effective bundle IDs (the signing area rewrites entitlements with it).
    pub bundle_ids: BTreeMap<String, String>,
    pub signing_plan: SigningPlan,
    pub dsyms: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ipa: Option<String>,
    /// `build-report.json`.
    pub report_path: String,
    pub lints: Vec<LintFinding>,
    #[ts(type = "number")]
    pub duration_ms: u64,
}

/// Result of `build.start`.
pub type BuildStartResult = BuildOutput;
crate::rpc_method!(
    /// `build.start` (class `build`, long-running). W5 (Wave 1).
    BuildStart,
    "build.start",
    BuildStartParams => BuildStartResult,
    Permission::Build, long_running
);

/// Params of `build.plan`: those of `build.start`.
pub type BuildPlanParams = BuildStartParams;

/// The SDK a plan resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanSdk {
    /// `xcode-27.0-27A266a`.
    pub id: String,
    /// `iphoneos27.0`.
    pub canonical_name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub build: Option<String>,
    pub xcode_version: String,
    pub xcode_build: String,
    pub swift_required: String,
    /// The bundle the SDK was read from.
    pub bundle: String,
}

/// Result of `build.plan`: what a build would do (no tool runs, nothing is written into the project).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BuildPlanResult {
    pub root: String,
    pub stack: ProjectStack,
    /// The app product; omitted when only the package description can tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub product: Option<String>,
    pub extensions: Vec<String>,
    pub configuration: String,
    pub release: bool,
    pub backend: BuildBackend,
    pub package: PackageKind,
    /// `none`, `minimal`, `sdk` or `xcode-compat` (ARCHITECTURE §16.4).
    pub metadata: String,
    pub sdk: PlanSdk,
    /// `[app] deployment-target`; omitted when the package's `.iOS` platform decides at build time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub deployment_target: Option<String>,
    pub bundle_ids: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub app_path: Option<String>,
    pub report_path: String,
    /// `resolve`, `prepare`, `compile`, `resources`, `assemble`, `lint`, `package`.
    pub stages: Vec<String>,
    pub warnings: Vec<String>,
}
crate::rpc_method!(
    /// `build.plan` (class `read`). W5 (Wave 1).
    BuildPlan,
    "build.plan",
    BuildPlanParams => BuildPlanResult,
    Permission::Read
);

/// Params of `build.settings`: `{project, target?, configuration?, keys?} → {settings, sources}`.
pub type BuildSettingsParams = Unspecified;
/// Result of `build.settings`.
pub type BuildSettingsResult = Unspecified;
crate::rpc_method!(
    /// `build.settings` (class `read`). W16 (Wave 2).
    BuildSettings,
    "build.settings",
    BuildSettingsParams => BuildSettingsResult,
    Permission::Read
);

/// What `build.clean` removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CleanScope {
    /// `.russet/work` and `.russet/build`.
    Build,
    /// All of `.russet`, logs included.
    All,
}

/// Params of `build.clean`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BuildCleanParams {
    pub project: String,
    pub scope: CleanScope,
}

/// Result of `build.clean`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BuildCleanResult {
    #[ts(type = "number")]
    pub freed_bytes: u64,
}
crate::rpc_method!(
    /// `build.clean` (class `build`). W5 (Wave 1).
    BuildClean,
    "build.clean",
    BuildCleanParams => BuildCleanResult,
    Permission::Build
);

/// Params of `build.inspect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BuildInspectParams {
    /// An `.app` directory or an `.ipa` file.
    pub path: String,
}

/// One Mach-O file of an inspected bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct MachOSummary {
    /// Relative to the bundle's parent (`RussetProbe.app/RussetProbe`).
    pub path: String,
    pub archs: Vec<String>,
    /// `LC_BUILD_VERSION` platform (2 = iOS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub platform: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub minos: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sdk: Option<String>,
    /// `lld 21.1.6`, …
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub uuid: Option<String>,
    pub rpaths: Vec<String>,
    pub dylibs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub encrypted: Option<bool>,
}

/// Result of `build.inspect`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BuildInspectResult {
    /// The Info.plist's JSON form (as `project.infoPlist`'s `plist`).
    pub info_plist: serde_json::Value,
    pub mach_o: Vec<MachOSummary>,
    pub lints: Vec<LintFinding>,
    /// The DT* keys.
    pub dt: BTreeMap<String, String>,
    /// The asset catalog summary; omitted until the CAR reader exists (W21).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub car: Option<serde_json::Value>,
}
crate::rpc_method!(
    /// `build.inspect` (class `read`). W5 (Wave 1).
    BuildInspect,
    "build.inspect",
    BuildInspectParams => BuildInspectResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] =
    &[BuildStart::INFO, BuildPlan::INFO, BuildSettings::INFO, BuildClean::INFO, BuildInspect::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
