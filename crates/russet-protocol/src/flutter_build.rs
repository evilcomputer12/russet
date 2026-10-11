// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `flutter.*` build half: registration, custom-device install, gen_snapshot. Owner: W17 (Wave 3), W24 (genSnapshot rows).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.3 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `flutter.register`: `{udid?, all?} → {registered[{udid, flutterDeviceId, configPath}]}`.
pub type FlutterRegisterParams = Unspecified;
/// Result of `flutter.register`.
pub type FlutterRegisterResult = Unspecified;
crate::rpc_method!(
    /// `flutter.register` (class `system`). W17 (Wave 3).
    FlutterRegister,
    "flutter.register",
    FlutterRegisterParams => FlutterRegisterResult,
    Permission::System
);

/// Params of `flutter.device.install`: `{udid, appName, bundlePath} → {bundleId}`.
pub type FlutterDeviceInstallParams = Unspecified;
/// Result of `flutter.device.install`.
pub type FlutterDeviceInstallResult = Unspecified;
crate::rpc_method!(
    /// `flutter.device.install` (class `device`, long-running). W17 (Wave 3).
    FlutterDeviceInstall,
    "flutter.device.install",
    FlutterDeviceInstallParams => FlutterDeviceInstallResult,
    Permission::Device, long_running
);

/// Params of `flutter.genSnapshot.ensure`: `{flutterRoot?, mode} → {path, engine, sha256}`.
pub type FlutterGenSnapshotEnsureParams = Unspecified;
/// Result of `flutter.genSnapshot.ensure`.
pub type FlutterGenSnapshotEnsureResult = Unspecified;
crate::rpc_method!(
    /// `flutter.genSnapshot.ensure` (class `system`, long-running). W24 (Wave 4).
    FlutterGenSnapshotEnsure,
    "flutter.genSnapshot.ensure",
    FlutterGenSnapshotEnsureParams => FlutterGenSnapshotEnsureResult,
    Permission::System, long_running
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[FlutterRegister::INFO, FlutterDeviceInstall::INFO, FlutterGenSnapshotEnsure::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
