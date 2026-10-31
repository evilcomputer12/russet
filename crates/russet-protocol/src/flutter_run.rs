// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `flutter.*` run half: Flutter sessions and the custom-device run hooks. Owner: W18.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.5 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `flutter.run`: `{project, udid?, mode?, args?, sessionLease?} → {sessionId, vmService{wsUri, httpUri}, devtoolsUri?}`.
pub type FlutterRunParams = Unspecified;
/// Result of `flutter.run`.
pub type FlutterRunResult = Unspecified;
crate::rpc_method!(
    /// `flutter.run` (class `device`, long-running). W18 (Wave 3).
    FlutterRun,
    "flutter.run",
    FlutterRunParams => FlutterRunResult,
    Permission::Device, long_running
);

/// Params of `flutter.attach`: `{sessionId} → {vmService}`.
pub type FlutterAttachParams = Unspecified;
/// Result of `flutter.attach`.
pub type FlutterAttachResult = Unspecified;
crate::rpc_method!(
    /// `flutter.attach` (class `device`). W18 (Wave 3).
    FlutterAttach,
    "flutter.attach",
    FlutterAttachParams => FlutterAttachResult,
    Permission::Device
);

/// Params of `flutter.device.runDebug`: `{udid, appName, mode, engineOptions} → {sessionId}`.
pub type FlutterDeviceRunDebugParams = Unspecified;
/// Result of `flutter.device.runDebug`.
pub type FlutterDeviceRunDebugResult = Unspecified;
crate::rpc_method!(
    /// `flutter.device.runDebug` (class `device`, long-running). W18 (Wave 3).
    FlutterDeviceRunDebug,
    "flutter.device.runDebug",
    FlutterDeviceRunDebugParams => FlutterDeviceRunDebugResult,
    Permission::Device, long_running
);

/// Params of `flutter.device.forward`: `{udid, devicePort, hostPort} → {leaseId, local}`.
pub type FlutterDeviceForwardParams = Unspecified;
/// Result of `flutter.device.forward`.
pub type FlutterDeviceForwardResult = Unspecified;
crate::rpc_method!(
    /// `flutter.device.forward` (class `device`). W18 (Wave 3).
    FlutterDeviceForward,
    "flutter.device.forward",
    FlutterDeviceForwardParams => FlutterDeviceForwardResult,
    Permission::Device
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] =
    &[FlutterRun::INFO, FlutterAttach::INFO, FlutterDeviceRunDebug::INFO, FlutterDeviceForward::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
