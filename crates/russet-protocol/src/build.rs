// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `build.*`: builds (ARCHITECTURE §8.3). Owner: W5 (Wave 1), W16 (build.settings).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.3 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `build.start`: `{project, target?, configuration?, package?: none|ipa-dev|ipa-store, backend?, overrides?, clean?, bundleIdPolicy?} → BuildOutput`.
pub type BuildStartParams = Unspecified;
/// Result of `build.start`.
pub type BuildStartResult = Unspecified;
crate::rpc_method!(
    /// `build.start` (class `build`, long-running). W5 (Wave 1).
    BuildStart,
    "build.start",
    BuildStartParams => BuildStartResult,
    Permission::Build, long_running
);

/// Params of `build.plan`: `as build.start → BuildPlan`.
pub type BuildPlanParams = Unspecified;
/// Result of `build.plan`.
pub type BuildPlanResult = Unspecified;
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

/// Params of `build.clean`: `{project, scope: build|all} → {freedBytes}`.
pub type BuildCleanParams = Unspecified;
/// Result of `build.clean`.
pub type BuildCleanResult = Unspecified;
crate::rpc_method!(
    /// `build.clean` (class `build`). W5 (Wave 1).
    BuildClean,
    "build.clean",
    BuildCleanParams => BuildCleanResult,
    Permission::Build
);

/// Params of `build.inspect`: `{path: .app|.ipa} → {infoPlist, machO[], lints[], dt, car?}`.
pub type BuildInspectParams = Unspecified;
/// Result of `build.inspect`.
pub type BuildInspectResult = Unspecified;
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
