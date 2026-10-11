// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `toolchain.*`: managed toolchains and pins (ARCHITECTURE §16). Owner: W3.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.1 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `toolchain.list`: `{project?, refresh?} → {tools: ToolInfo[], sdks: SdkInfo[], pins{id,source}}`.
pub type ToolchainListParams = Unspecified;
/// Result of `toolchain.list`.
pub type ToolchainListResult = Unspecified;
crate::rpc_method!(
    /// `toolchain.list` (class `read`). W3 (Wave 1).
    ToolchainList,
    "toolchain.list",
    ToolchainListParams => ToolchainListResult,
    Permission::Read
);

/// Params of `toolchain.resolve`: `{component, project?} → ToolInfo (incl. env)`.
pub type ToolchainResolveParams = Unspecified;
/// Result of `toolchain.resolve`.
pub type ToolchainResolveResult = Unspecified;
crate::rpc_method!(
    /// `toolchain.resolve` (class `read`). W3 (Wave 1).
    ToolchainResolve,
    "toolchain.resolve",
    ToolchainResolveParams => ToolchainResolveResult,
    Permission::Read
);

/// Params of `toolchain.install`: `{component, version} → ToolInfo`.
pub type ToolchainInstallParams = Unspecified;
/// Result of `toolchain.install`.
pub type ToolchainInstallResult = Unspecified;
crate::rpc_method!(
    /// `toolchain.install` (class `system`, long-running). W3 (Wave 1: python-shared, darwin-tools).
    ToolchainInstall,
    "toolchain.install",
    ToolchainInstallParams => ToolchainInstallResult,
    Permission::System, long_running
);

/// Params of `toolchain.uninstall`: `{component, version} → {freedBytes}`.
pub type ToolchainUninstallParams = Unspecified;
/// Result of `toolchain.uninstall`.
pub type ToolchainUninstallResult = Unspecified;
crate::rpc_method!(
    /// `toolchain.uninstall` (class `system`). W3 (Wave 2).
    ToolchainUninstall,
    "toolchain.uninstall",
    ToolchainUninstallParams => ToolchainUninstallResult,
    Permission::System
);

/// Params of `toolchain.gc`: `{dryRun?} → {freedBytes, removed[]}`.
pub type ToolchainGcParams = Unspecified;
/// Result of `toolchain.gc`.
pub type ToolchainGcResult = Unspecified;
crate::rpc_method!(
    /// `toolchain.gc` (class `system`). W3 (Wave 2).
    ToolchainGc,
    "toolchain.gc",
    ToolchainGcParams => ToolchainGcResult,
    Permission::System
);

/// Params of `toolchain.pins.update`: `{channel?} → {id, changed}`.
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

/// Params of the `toolchain.changed` notification: `{components[]}; to all`.
pub type ToolchainChangedParams = Unspecified;
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
