// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `app.*`: installed apps, install, launch, terminate (ARCHITECTURE §8.2). Owner: W4 (Wave 1), W10 (app.icon).

use std::collections::BTreeMap;

use russet_core::ids::LeaseId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    ClientKind, ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod, RpcNotification, Unspecified,
};

/// Which apps `app.list` returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AppKind {
    /// Apps the user installed (the default).
    #[default]
    User,
    /// Every app, system apps included.
    All,
}

/// Params of `app.list`: `{udid?, kind?: user|all, bundleIdPrefix?} → {apps: AppInfo[]}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<AppKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id_prefix: Option<String>,
}

/// One installed app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub bundle_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// `CFBundleShortVersionString`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version: Option<String>,
    /// `CFBundleVersion`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub build: Option<String>,
    /// The `.app` path on the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    /// `CFBundleExecutable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub executable: Option<String>,
    /// A user app with `get-task-allow` (debuggable).
    pub developer_app: bool,
    pub removable: bool,
    pub hidden: bool,
}

/// Result of `app.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppListResult {
    pub apps: Vec<AppInfo>,
}
crate::rpc_method!(
    /// `app.list` (class `read`). W4 (Wave 1).
    AppList,
    "app.list",
    AppListParams => AppListResult,
    Permission::Read
);

/// Params of `app.install`: `{udid?, path: .app|.ipa} → {bundleId, installedPath, durationMs}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppInstallParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// An absolute path to a signed `.app` directory or `.ipa`.
    pub path: String,
}

/// Result of `app.install`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppInstallResult {
    pub bundle_id: String,
    /// The app's path on the device (empty when installd did not report it).
    pub installed_path: String,
    #[ts(type = "number")]
    pub duration_ms: u64,
}
crate::rpc_method!(
    /// `app.install` (class `device`, long-running). W4 (Wave 1).
    AppInstall,
    "app.install",
    AppInstallParams => AppInstallResult,
    Permission::Device, long_running
);

/// Params of `app.uninstall`: `{udid?, bundleId} → {} (MCP: own or dev.russet.test.* IDs only)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppUninstallParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// Required; `#[serde(default)]` only so the per-call class can be resolved before validation (the handler
    /// rejects an empty id with `core.invalidArgument`).
    #[serde(default)]
    pub bundle_id: String,
}

/// Result of `app.uninstall`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppUninstallResult {}
crate::rpc_method!(
    /// `app.uninstall` (class `device`; from MCP only the project's own or `dev.russet.test.*` bundle IDs, else
    /// `perm.denied`; resolved per call). W4 (Wave 1).
    AppUninstall,
    "app.uninstall",
    AppUninstallParams => AppUninstallResult,
    Permission::Device, permission_fn = app_uninstall_permission
);

/// The bundle-ID prefix agents may always uninstall [D 55].
pub const TEST_BUNDLE_PREFIX: &str = "dev.russet.test.";

/// Per-call class of `app.uninstall` [D 55, D 66]: an MCP connection may uninstall only the bundle IDs of its
/// project ([`Ownership::project_bundle_ids`]) and `dev.russet.test.*`; anything else is `perm.denied`. Other
/// clients get `device`.
pub fn app_uninstall_permission(
    p: &AppUninstallParams,
    c: &ConnInfo,
    o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    if c.kind == ClientKind::Mcp {
        let own = o.project_bundle_ids(&c.id).iter().any(|b| b == &p.bundle_id);
        if !own && !p.bundle_id.starts_with(TEST_BUNDLE_PREFIX) {
            return Err(russet_core::Error::new(
                russet_core::error::codes::perm::DENIED,
                format!(
                    "app.uninstall from an MCP client is limited to the project's own bundle IDs and {TEST_BUNDLE_PREFIX}*; {} is neither",
                    p.bundle_id
                ),
            )
            .with_hint("Ask the user to uninstall the app, or run `russet app uninstall` yourself."));
        }
    }
    Ok(Permission::Device)
}

/// Params of `app.launch`: `{udid?, bundleId, args?, env?, suspended?, killExisting?, console?} → {pid, executable, leaseId} ⇒ app.output, app.exited`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppLaunchParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    pub bundle_id: String,
    /// Arguments after the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Start stopped before `main` (resume with `process.signal` SIGCONT or a debugger).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub suspended: Option<bool>,
    /// Terminate a running instance first (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kill_existing: Option<bool>,
    /// Attach the app's stdio (default true): `app.output` notifications and the `stdout` log source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub console: Option<bool>,
}

/// Result of `app.launch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppLaunchResult {
    pub pid: u32,
    /// The executable's path on the device (empty after a DVT launch).
    pub executable: String,
    /// The `app-launch` lease: console and exit notifications go to the caller until it ends.
    pub lease_id: LeaseId,
}
crate::rpc_method!(
    /// `app.launch` (class `device`). W4 (Wave 1).
    AppLaunch,
    "app.launch",
    AppLaunchParams => AppLaunchResult,
    Permission::Device
);

/// Params of `app.terminate`: `{udid?, pid?|bundleId?} → {}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppTerminateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
}

/// Result of `app.terminate`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppTerminateResult {}
crate::rpc_method!(
    /// `app.terminate` (class `device`). W4 (Wave 1).
    AppTerminate,
    "app.terminate",
    AppTerminateParams => AppTerminateResult,
    Permission::Device
);

/// Params of `app.icon`: `{udid?, bundleId} → {pngBase64}`.
pub type AppIconParams = Unspecified;
/// Result of `app.icon`.
pub type AppIconResult = Unspecified;
crate::rpc_method!(
    /// `app.icon` (class `read`). W10 (Wave 2).
    AppIcon,
    "app.icon",
    AppIconParams => AppIconResult,
    Permission::Read
);

/// The stream of an `app.output` chunk (CoreDevice merges stdout and stderr on one pseudo-terminal).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AppOutputStream {
    #[default]
    Stdout,
}

/// Params of the `app.output` notification: `{leaseId, stream: stdout, data}; to the app.launch caller`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppOutputParams {
    pub lease_id: LeaseId,
    pub stream: AppOutputStream,
    /// One console line, without its line ending.
    pub data: String,
}
crate::rpc_notification!(
    /// `app.output` (daemon → client). W4.
    AppOutput,
    "app.output",
    AppOutputParams
);

/// Params of the `app.exited` notification: `{leaseId, pid, code?, signal?, crashId?}; to the app.launch caller`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppExitedParams {
    pub lease_id: LeaseId,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub signal: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub crash_id: Option<String>,
}
crate::rpc_notification!(
    /// `app.exited` (daemon → client). W4.
    AppExited,
    "app.exited",
    AppExitedParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] =
    &[AppList::INFO, AppInstall::INFO, AppUninstall::INFO, AppLaunch::INFO, AppTerminate::INFO, AppIcon::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[AppOutput::INFO, AppExited::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
