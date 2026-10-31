// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `device.*`: discovery, readiness, pairing, DDI, reservation (ARCHITECTURE §8.2). Owner: W4 (Wave 1), W10 (Wave 2), W30 (Wi-Fi).
//!
//! The shared shapes (`DeviceSummary`, `Readiness`, `DdiStatus`) are in [`crate::common`] (ARCHITECTURE §9.5).
//! W10's rows (`device.unpair`, `device.developerMode.reveal`, `device.ddi.list`) and W30's `device.wifi.enable`
//! keep placeholder DTOs ([`Unspecified`]) until their cards.

use std::collections::BTreeMap;

use russet_core::ids::{LeaseId, SubscriptionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{CodeSigningMonitor, DdiStatus, DeviceState, DeviceSummary, DeviceTransport, Readiness};
use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `device.list`: `{} → {devices: DeviceSummary[]}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceListParams {}

/// Result of `device.list`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceListResult {
    pub devices: Vec<DeviceSummary>,
}
crate::rpc_method!(
    /// `device.list` (class `read`). W4 (Wave 1).
    DeviceList,
    "device.list",
    DeviceListParams => DeviceListResult,
    Permission::Read
);

/// Params of `device.watch`: `{} → {subscriptionId} ⇒ device.changed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceWatchParams {}

/// Result of `device.watch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceWatchResult {
    pub subscription_id: SubscriptionId,
}
crate::rpc_method!(
    /// `device.watch` (class `read`). W4 (Wave 1).
    DeviceWatch,
    "device.watch",
    DeviceWatchParams => DeviceWatchResult,
    Permission::Read
);

/// Params of `device.info`: `{udid?} → DeviceInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfoParams {
    /// A UDID or alias; may be omitted when exactly one device is attached (ARCHITECTURE §7.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
}

/// Result of `device.info`: lockdown identity values plus a few safe properties (never serial numbers, ECIDs or
/// network addresses).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfoResult {
    pub udid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub product_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub marketing_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub os_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub build: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hardware_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cpu_architecture: Option<String>,
    /// `ChipID`, for example 33025 (0x8101).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub chip_id: Option<u32>,
    pub transport: DeviceTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code_signing_monitor: Option<CodeSigningMonitor>,
    pub state: DeviceState,
    /// Further lockdown values (`ProductName`, `ActivationState`, `PasswordProtected`, …).
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
}
crate::rpc_method!(
    /// `device.info` (class `read`). W4 (Wave 1).
    DeviceInfo,
    "device.info",
    DeviceInfoParams => DeviceInfoResult,
    Permission::Read
);

/// Params of `device.readiness`: `{udid?, refresh?} → Readiness {paired, developerMode, ddi, locked, tunnel}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceReadinessParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// Re-read everything instead of answering from the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// Result of `device.readiness`.
pub type DeviceReadinessResult = Readiness;
crate::rpc_method!(
    /// `device.readiness` (class `read`). W4 (Wave 1).
    DeviceReadiness,
    "device.readiness",
    DeviceReadinessParams => DeviceReadinessResult,
    Permission::Read
);

/// Where `device.prepare` and `device.ddi.mount` take the DDI from (ARCHITECTURE §8.2, §14.3) [D 62].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DdiSourceParam {
    /// An imported DDI (`data/ddi/<ddiBuild>/iOS`); the newest when `ddiBuild` is omitted (W10).
    Imported {
        #[serde(default, rename = "ddiBuild", skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        ddi_build: Option<String>,
    },
    /// A DDI root holding `Restore/` and `version.plist`, or a directory holding `BuildManifest.plist` [D 82].
    Dir { path: String },
    /// pymobiledevice3's mirror-derived cache; needs the `allow-mirror` consent (W10).
    Pmd3Cache {},
    /// The opt-in `pymobiledevice3 mounter auto-mount` subprocess (W10).
    Pmd3AutoMount {
        #[serde(rename = "allowMirror")]
        allow_mirror: bool,
    },
}

/// Params of `device.prepare`: `{udid?, ddiSource?, revealDeveloperMode?} → Readiness`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DevicePrepareParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// Default: the `[device.ddi]` order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ddi_source: Option<DdiSourceParam>,
    /// Reveal the Developer Mode toggle in Settings when it is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reveal_developer_mode: Option<bool>,
}

/// Result of `device.prepare`.
pub type DevicePrepareResult = Readiness;
crate::rpc_method!(
    /// `device.prepare` (class `device`, long-running). W4 (DDI from a directory) / W10 (imported DDI default) (Wave 1).
    DevicePrepare,
    "device.prepare",
    DevicePrepareParams => DevicePrepareResult,
    Permission::Device, long_running
);

/// Params of `device.pair`: `{udid?, hostName?} → {status: paired|pending|denied}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DevicePairParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// The name the Trust dialog shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub host_name: Option<String>,
}

/// The outcome of `device.pair`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum PairStatus {
    Paired,
    Pending,
    Denied,
}

/// Result of `device.pair`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DevicePairResult {
    pub status: PairStatus,
}
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDdiStatusParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
}

/// Result of `device.ddi.status`.
pub type DeviceDdiStatusResult = DdiStatus;
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDdiMountParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// Default: the `[device.ddi]` order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<DdiSourceParam>,
    /// Replace a mounted DDI (on the rig only with the owner's approval [D 62]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub force: Option<bool>,
}

/// Result of `device.ddi.mount`.
pub type DeviceDdiMountResult = DdiStatus;
crate::rpc_method!(
    /// `device.ddi.mount` (class `device`, long-running). W4 (dir, Wave 1) / W10 (the rest, Wave 2).
    DeviceDdiMount,
    "device.ddi.mount",
    DeviceDdiMountParams => DeviceDdiMountResult,
    Permission::Device, long_running
);

/// Params of `device.reserve`: `{udid?, purpose} → {leaseId}; others get device.reserved`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceReserveParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// Shown to the clients that get `device.reserved`.
    #[serde(default)]
    pub purpose: String,
}

/// Result of `device.reserve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceReserveResult {
    pub lease_id: LeaseId,
}
crate::rpc_method!(
    /// `device.reserve` (class `device`). W4 (Wave 1).
    DeviceReserve,
    "device.reserve",
    DeviceReserveParams => DeviceReserveResult,
    Permission::Device
);

/// Params of `device.alias.set`: `{udid, alias} → {} (global config)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAliasSetParams {
    /// The UDID; an empty string removes the alias.
    pub udid: String,
    pub alias: String,
}

/// Result of `device.alias.set`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAliasSetResult {}
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

/// What happened to a device (`device.changed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceChange {
    Added,
    Removed,
    Updated,
}

/// Params of the `device.changed` notification: `{subscriptionId, change: added|removed|updated, device: DeviceSummary}; to subscribers`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceChangedParams {
    pub subscription_id: SubscriptionId,
    pub change: DeviceChange,
    pub device: DeviceSummary,
}
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
