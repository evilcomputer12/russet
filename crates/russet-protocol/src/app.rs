// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `app.*`: installed apps, install, launch, terminate. Owner: W4 (Wave 1), W10 (app.icon).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.2 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{
    ClientKind, ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod, RpcNotification, Unspecified,
};

/// Params of `app.list`: `{udid?, kind?: user|all, bundleIdPrefix?} → {apps: AppInfo[]}`.
pub type AppListParams = Unspecified;
/// Result of `app.list`.
pub type AppListResult = Unspecified;
crate::rpc_method!(
    /// `app.list` (class `read`). W4 (Wave 1).
    AppList,
    "app.list",
    AppListParams => AppListResult,
    Permission::Read
);

/// Params of `app.install`: `{udid?, path: .app|.ipa} → {bundleId, installedPath, durationMs}`.
pub type AppInstallParams = Unspecified;
/// Result of `app.install`.
pub type AppInstallResult = Unspecified;
crate::rpc_method!(
    /// `app.install` (class `device`, long-running). W4 (Wave 1).
    AppInstall,
    "app.install",
    AppInstallParams => AppInstallResult,
    Permission::Device, long_running
);

/// Params of `app.uninstall`: `{udid?, bundleId} → {} (MCP: own or dev.russet.test.* IDs only)`.
pub type AppUninstallParams = Unspecified;
/// Result of `app.uninstall`.
pub type AppUninstallResult = Unspecified;
crate::rpc_method!(
    /// `app.uninstall` (class `device`). W4 (Wave 1).
    AppUninstall,
    "app.uninstall",
    AppUninstallParams => AppUninstallResult,
    Permission::Device, permission_fn = app_uninstall_permission
);

/// Per-call class of `app.uninstall` [D 66]: for `mcp` connections only the project's own bundle IDs or `dev.russet.test.*`, else `perm.denied`.
///
/// F0 stub: fails closed. MCP connections are denied until the owner checks the bundle ID against
/// [`Ownership::project_bundle_ids`] and the `dev.russet.test.` prefix; other clients get `device`.
pub fn app_uninstall_permission(
    _p: &AppUninstallParams,
    c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    if c.kind == ClientKind::Mcp {
        return Err(russet_core::Error::new(
            russet_core::error::codes::perm::DENIED,
            "app.uninstall from an MCP client is limited to the project's own bundle IDs and dev.russet.test.*",
        ));
    }
    Ok(Permission::Device)
}

/// Params of `app.launch`: `{udid?, bundleId, args?, env?, suspended?, killExisting?, console?} → {pid, executable, leaseId} ⇒ app.output, app.exited`.
pub type AppLaunchParams = Unspecified;
/// Result of `app.launch`.
pub type AppLaunchResult = Unspecified;
crate::rpc_method!(
    /// `app.launch` (class `device`). W4 (Wave 1).
    AppLaunch,
    "app.launch",
    AppLaunchParams => AppLaunchResult,
    Permission::Device
);

/// Params of `app.terminate`: `{udid?, pid?|bundleId?} → {}`.
pub type AppTerminateParams = Unspecified;
/// Result of `app.terminate`.
pub type AppTerminateResult = Unspecified;
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

/// Params of the `app.output` notification: `{leaseId, stream: stdout, data}; to the app.launch caller`.
pub type AppOutputParams = Unspecified;
crate::rpc_notification!(
    /// `app.output` (daemon → client). W4.
    AppOutput,
    "app.output",
    AppOutputParams
);

/// Params of the `app.exited` notification: `{leaseId, pid, code?, signal?, crashId?}; to the app.launch caller`.
pub type AppExitedParams = Unspecified;
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
