// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `toolchain.*`: resolved tools, managed installs and pins (ARCHITECTURE §8.1, §16). Owner: W3 (`pins.update`:
//! W26).
//!
//! `ToolInfo.env` is part of the contract (§16.2): every launcher applies it (LLDB needs `LD_LIBRARY_PATH` and
//! `PYTHONHOME` for libpython).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// A toolchain component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ToolComponent {
    Swift,
    DarwinSdk,
    Linker,
    Lldb,
    LldbDap,
    Libpython,
    Xtool,
    Flutter,
    GenSnapshotIos,
    Llvm,
}

/// A component Russet downloads and installs itself (`toolchain.install`). Wave 1: `python-shared` and
/// `darwin-tools`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ManagedComponent {
    /// python-build-standalone, the libpython of Swift's LLDB.
    PythonShared,
    /// The darwin-tools linker (`ld64.lld`, LLD 21.1.6 patched for `arm64e.x1` TBDs).
    DarwinTools,
    Swift,
    Llvm,
    Flutter,
    Xtool,
}

/// Where a resolved tool came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ToolProvenance {
    /// Installed by Russet; `pin` is `<component>@<version>`.
    Managed { pin: String },
    /// Found on the system (`PATH`, `ldconfig`).
    System,
    /// An existing uv-managed python-build-standalone.
    Uv,
    /// Inside an xtool SDK bundle, read in place.
    #[serde(rename_all = "camelCase")]
    XtoolBundle { bundle: String },
    /// Installed by the user elsewhere (swiftly).
    User,
    /// Named by the project (`[toolchain] linker = "<path>"`, `[debug] lldb = "<path>"`).
    ProjectPin,
}

/// A resolved tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub component: ToolComponent,
    pub version: String,
    pub path: String,
    pub provenance: ToolProvenance,
    /// Environment every launcher must apply (ARCHITECTURE §16.2).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub ok: bool,
    #[serde(default)]
    pub problems: Vec<String>,
}

/// A Darwin SDK known to the registry (Russet imports and xtool bundles).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SdkInfo {
    /// For example `xcode-27.0-27A266a`.
    pub id: String,
    pub xcode_version: String,
    pub xcode_build: String,
    pub ios_sdk: String,
    pub swift_sdk_name: String,
    pub bundle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub darwin_tools: Option<String>,
    /// The Swift `major.minor` the SDK needs (empty when the pins have no row for it).
    pub compatible_swift: String,
}

/// The pin manifest in use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PinsInfo {
    pub id: String,
    /// `embedded`, or the path of an updated manifest.
    pub source: String,
}

/// Params of `toolchain.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainListParams {
    /// The project root, for its `[toolchain]` and `[debug]` pins: candidates they exclude are listed with
    /// `ok: false` and the reason in `problems`, and a tool the project names by path comes first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    /// Re-run every probe instead of using the probe cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// Result of `toolchain.list`: every candidate of every component, in resolution order; the first `ok` candidate
/// of a component is the one `toolchain.resolve` picks (for the given project).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainListResult {
    pub tools: Vec<ToolInfo>,
    pub sdks: Vec<SdkInfo>,
    pub pins: PinsInfo,
}

crate::rpc_method!(
    /// `toolchain.list` (class `read`). W3 (Wave 1).
    ToolchainList,
    "toolchain.list",
    ToolchainListParams => ToolchainListResult,
    Permission::Read
);

/// Params of `toolchain.resolve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainResolveParams {
    pub component: ToolComponent,
    /// The project root, for `[toolchain]` and `[debug] lldb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
}

/// Result of `toolchain.resolve`: the first working candidate, with its env.
pub type ToolchainResolveResult = ToolInfo;

crate::rpc_method!(
    /// `toolchain.resolve` (class `read`). W3 (Wave 1).
    ToolchainResolve,
    "toolchain.resolve",
    ToolchainResolveParams => ToolchainResolveResult,
    Permission::Read
);

/// Params of `toolchain.install`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainInstallParams {
    pub component: ManagedComponent,
    /// The pinned version; omitted = the pin manifest's version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version: Option<String>,
}

/// Result of `toolchain.install`: the installed tool, resolved.
pub type ToolchainInstallResult = ToolInfo;

crate::rpc_method!(
    /// `toolchain.install` (class `system`, long-running). W3 (Wave 1: python-shared, darwin-tools).
    ToolchainInstall,
    "toolchain.install",
    ToolchainInstallParams => ToolchainInstallResult,
    Permission::System, long_running
);

/// Params of `toolchain.uninstall`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainUninstallParams {
    pub component: ManagedComponent,
    pub version: String,
}

/// Result of `toolchain.uninstall`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainUninstallResult {
    #[ts(type = "number")]
    pub freed_bytes: u64,
}

crate::rpc_method!(
    /// `toolchain.uninstall` (class `system`). W3 (Wave 2).
    ToolchainUninstall,
    "toolchain.uninstall",
    ToolchainUninstallParams => ToolchainUninstallResult,
    Permission::System
);

/// Params of `toolchain.gc`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainGcParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dry_run: Option<bool>,
}

/// Result of `toolchain.gc`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainGcResult {
    #[ts(type = "number")]
    pub freed_bytes: u64,
    pub removed: Vec<String>,
}

crate::rpc_method!(
    /// `toolchain.gc` (class `system`). W3 (Wave 2).
    ToolchainGc,
    "toolchain.gc",
    ToolchainGcParams => ToolchainGcResult,
    Permission::System
);

/// Params of `toolchain.pins.update`: `{channel?} → {id, changed}` (W26 defines them).
pub type ToolchainPinsUpdateParams = Unspecified;
/// Result of `toolchain.pins.update`.
pub type ToolchainPinsUpdateResult = Unspecified;
crate::rpc_method!(
    /// `toolchain.pins.update` (class `system`). W26 (Wave 4).
    ToolchainPinsUpdate,
    "toolchain.pins.update",
    ToolchainPinsUpdateParams => ToolchainPinsUpdateResult,
    Permission::System
);

/// Params of the `toolchain.changed` notification (to every connection).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainChangedParams {
    pub components: Vec<ToolComponent>,
}

crate::rpc_notification!(
    /// `toolchain.changed` (daemon → client). W3.
    ToolchainChanged,
    "toolchain.changed",
    ToolchainChangedParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[
    ToolchainList::INFO,
    ToolchainResolve::INFO,
    ToolchainInstall::INFO,
    ToolchainUninstall::INFO,
    ToolchainGc::INFO,
    ToolchainPinsUpdate::INFO,
];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[ToolchainChanged::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
