// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `device.*`: discovery, readiness, pairing, DDI, reservation (ARCHITECTURE §8.2). Owner: W4 (Wave 1), W10 (Wave 2), W30 (Wi-Fi).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.2 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `device.list`: `{} → {devices: DeviceSummary[]}`.
pub type DeviceListParams = Unspecified;
/// Result of `device.list`.
pub type DeviceListResult = Unspecified;
crate::rpc_method!(
    /// `device.list` (class `read`). W4 (Wave 1).
    DeviceList,
    "device.list",
    DeviceListParams => DeviceListResult,
    Permission::Read
);

/// Params of `device.watch`: `{} → {subscriptionId} ⇒ device.changed`.
pub type DeviceWatchParams = Unspecified;
/// Result of `device.watch`.
pub type DeviceWatchResult = Unspecified;
crate::rpc_method!(
    /// `device.watch` (class `read`). W4 (Wave 1).
    DeviceWatch,
    "device.watch",
    DeviceWatchParams => DeviceWatchResult,
    Permission::Read
);

/// Params of `device.info`: `{udid?} → DeviceInfo`.
pub type DeviceInfoParams = Unspecified;
/// Result of `device.info`.
pub type DeviceInfoResult = Unspecified;
crate::rpc_method!(
    /// `device.info` (class `read`). W4 (Wave 1).
    DeviceInfo,
    "device.info",
    DeviceInfoParams => DeviceInfoResult,
    Permission::Read
);

/// Params of `device.readiness`: `{udid?, refresh?} → Readiness {paired, developerMode, ddi, locked, tunnel}`.
pub type DeviceReadinessParams = Unspecified;
/// Result of `device.readiness`.
pub type DeviceReadinessResult = Unspecified;
crate::rpc_method!(
    /// `device.readiness` (class `read`). W4 (Wave 1).
    DeviceReadiness,
    "device.readiness",
    DeviceReadinessParams => DeviceReadinessResult,
    Permission::Read
);

/// Params of `device.prepare`: `{udid?, ddiSource?, revealDeveloperMode?} → Readiness`.
pub type DevicePrepareParams = Unspecified;
/// Result of `device.prepare`.
pub type DevicePrepareResult = Unspecified;
crate::rpc_method!(
    /// `device.prepare` (class `device`, long-running). W4 (DDI from a directory) / W10 (imported DDI default) (Wave 1).
    DevicePrepare,
    "device.prepare",
    DevicePrepareParams => DevicePrepareResult,
    Permission::Device, long_running
);

/// Params of `device.pair`: `{udid?, hostName?} → {status: paired|pending|denied}`.
pub type DevicePairParams = Unspecified;
/// Result of `device.pair`.
pub type DevicePairResult = Unspecified;
crate::rpc_method!(
    /// `device.pair` (class `device`, long-running). W4 (Wave 1).
    DevicePair,
    "device.pair",
    DevicePairParams => DevicePairResult,
    Permission::Device, long_running
);

/// Params of `device.unpair`: `{udid} → {}`.
pub type DeviceUnpairParams = Unspecified;
/// Result of `device.unpair`.
pub type DeviceUnpairResult = Unspecified;
crate::rpc_method!(
    /// `device.unpair` (class `device`). W10 (Wave 2).
    DeviceUnpair,
    "device.unpair",
    DeviceUnpairParams => DeviceUnpairResult,
    Permission::Device
);

/// Params of `device.developerMode.reveal`: `{udid?} → {developerMode}`.
pub type DeviceDeveloperModeRevealParams = Unspecified;
/// Result of `device.developerMode.reveal`.
pub type DeviceDeveloperModeRevealResult = Unspecified;
crate::rpc_method!(
    /// `device.developerMode.reveal` (class `device`). W10 (Wave 2).
    DeviceDeveloperModeReveal,
    "device.developerMode.reveal",
    DeviceDeveloperModeRevealParams => DeviceDeveloperModeRevealResult,
    Permission::Device
);

/// Params of `device.ddi.status`: `{udid?} → DdiStatus {mounted, kind, version?, productBuild?, source?}`.
pub type DeviceDdiStatusParams = Unspecified;
/// Result of `device.ddi.status`.
pub type DeviceDdiStatusResult = Unspecified;
crate::rpc_method!(
    /// `device.ddi.status` (class `read`). W4 (Wave 1).
    DeviceDdiStatus,
    "device.ddi.status",
    DeviceDdiStatusParams => DeviceDdiStatusResult,
    Permission::Read
);

/// Params of `device.ddi.list`: `{platform?: ios} → {ddis: […]} (newest ddiBuild first)`.
pub type DeviceDdiListParams = Unspecified;
/// Result of `device.ddi.list`.
pub type DeviceDdiListResult = Unspecified;
crate::rpc_method!(
    /// `device.ddi.list` (class `read`). W10 (Wave 2).
    DeviceDdiList,
    "device.ddi.list",
    DeviceDdiListParams => DeviceDdiListResult,
    Permission::Read
);

/// Params of `device.ddi.mount`: `{udid?, source?: {kind: imported, ddiBuild?}|{kind: dir, path}|{kind: pmd3-cache}|{kind: pmd3-auto-mount, allowMirror}, force?} → DdiStatus`.
pub type DeviceDdiMountParams = Unspecified;
/// Result of `device.ddi.mount`.
pub type DeviceDdiMountResult = Unspecified;
crate::rpc_method!(
    /// `device.ddi.mount` (class `device`, long-running). W4 (dir, Wave 1) / W10 (the rest, Wave 2).
    DeviceDdiMount,
    "device.ddi.mount",
    DeviceDdiMountParams => DeviceDdiMountResult,
    Permission::Device, long_running
);

/// Params of `device.reserve`: `{udid?, purpose} → {leaseId}; others get device.reserved`.
pub type DeviceReserveParams = Unspecified;
/// Result of `device.reserve`.
pub type DeviceReserveResult = Unspecified;
crate::rpc_method!(
    /// `device.reserve` (class `device`). W4 (Wave 1).
    DeviceReserve,
    "device.reserve",
    DeviceReserveParams => DeviceReserveResult,
    Permission::Device
);

/// Params of `device.alias.set`: `{udid, alias} → {} (global config)`.
pub type DeviceAliasSetParams = Unspecified;
/// Result of `device.alias.set`.
pub type DeviceAliasSetResult = Unspecified;
crate::rpc_method!(
    /// `device.alias.set` (class `system`). W4 (Wave 1).
    DeviceAliasSet,
    "device.alias.set",
    DeviceAliasSetParams => DeviceAliasSetResult,
    Permission::System
);

/// Params of `device.wifi.enable`: `{udid} → {}`.
pub type DeviceWifiEnableParams = Unspecified;
/// Result of `device.wifi.enable`.
pub type DeviceWifiEnableResult = Unspecified;
crate::rpc_method!(
    /// `device.wifi.enable` (class `device`). W30 (Wave 6).
    DeviceWifiEnable,
    "device.wifi.enable",
    DeviceWifiEnableParams => DeviceWifiEnableResult,
    Permission::Device
);

/// Params of the `device.changed` notification: `{subscriptionId, change: added|removed|updated, device: DeviceSummary}; to subscribers`.
pub type DeviceChangedParams = Unspecified;
crate::rpc_notification!(
    /// `device.changed` (daemon → client). W4.
    DeviceChanged,
    "device.changed",
    DeviceChangedParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[
    DeviceList::INFO,
    DeviceWatch::INFO,
    DeviceInfo::INFO,
    DeviceReadiness::INFO,
    DevicePrepare::INFO,
    DevicePair::INFO,
    DeviceUnpair::INFO,
    DeviceDeveloperModeReveal::INFO,
    DeviceDdiStatus::INFO,
    DeviceDdiList::INFO,
    DeviceDdiMount::INFO,
    DeviceReserve::INFO,
    DeviceAliasSet::INFO,
    DeviceWifiEnable::INFO,
];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[DeviceChanged::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
