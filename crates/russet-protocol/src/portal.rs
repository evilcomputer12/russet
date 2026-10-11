// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `portal.*`: developer-portal records (ARCHITECTURE §8.4). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! The list rows read App Store Connect with GET-scoped tokens. UDIDs and device names are masked unless an
//! `ide` connection asks with `reveal: true`. The Wave-2 rows keep placeholder DTOs ([`Unspecified`]).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params shared by the `portal.*.list` rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PortalListParams {
    /// Omitted: the default account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub account_id: Option<String>,
    /// A filter value: the certificate type (`DEVELOPMENT`), the bundle-ID prefix, or the profile type
    /// (`IOS_APP_DEVELOPMENT`), depending on the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub filter: Option<String>,
    /// `portal.devices.list` from an `ide` connection: unmasked UDIDs and names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reveal: Option<bool>,
}

/// A portal certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PortalCertificate {
    pub id: String,
    /// `DEVELOPMENT`, `DISTRIBUTION`, … (open).
    pub certificate_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expires: Option<String>,
    /// Whether a local identity holds its private key.
    pub local_identity: bool,
}

/// Params of `portal.certificates.list`.
pub type PortalCertificatesListParams = PortalListParams;
/// Result of `portal.certificates.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PortalCertificatesListResult {
    pub certificates: Vec<PortalCertificate>,
}
crate::rpc_method!(
    /// `portal.certificates.list` (class `read`). W6 (Wave 1).
    PortalCertificatesList,
    "portal.certificates.list",
    PortalCertificatesListParams => PortalCertificatesListResult,
    Permission::Read
);

/// A registered device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PortalDevice {
    pub id: String,
    /// Masked (`0000…5F60`) unless revealed.
    pub udid: String,
    /// Omitted unless revealed (device names often name a person).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<String>,
}

/// Params of `portal.devices.list`.
pub type PortalDevicesListParams = PortalListParams;
/// Result of `portal.devices.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PortalDevicesListResult {
    pub devices: Vec<PortalDevice>,
}
crate::rpc_method!(
    /// `portal.devices.list` (class `read`). W6 (Wave 1).
    PortalDevicesList,
    "portal.devices.list",
    PortalDevicesListParams => PortalDevicesListResult,
    Permission::Read
);

/// A registered bundle ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PortalBundleId {
    pub id: String,
    pub identifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub platform: Option<String>,
    /// Enabled capability types (open strings).
    pub capabilities: Vec<String>,
}

/// Params of `portal.bundleIds.list`.
pub type PortalBundleIdsListParams = PortalListParams;
/// Result of `portal.bundleIds.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PortalBundleIdsListResult {
    pub bundle_ids: Vec<PortalBundleId>,
}
crate::rpc_method!(
    /// `portal.bundleIds.list` (class `read`). W6 (Wave 1).
    PortalBundleIdsList,
    "portal.bundleIds.list",
    PortalBundleIdsListParams => PortalBundleIdsListResult,
    Permission::Read
);

/// A portal profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PortalProfile {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// `IOS_APP_DEVELOPMENT`, … (open).
    pub profile_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expires: Option<String>,
}

/// Params of `portal.profiles.list`.
pub type PortalProfilesListParams = PortalListParams;
/// Result of `portal.profiles.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PortalProfilesListResult {
    pub profiles: Vec<PortalProfile>,
}
crate::rpc_method!(
    /// `portal.profiles.list` (class `read`). W6 (Wave 1).
    PortalProfilesList,
    "portal.profiles.list",
    PortalProfilesListParams => PortalProfilesListResult,
    Permission::Read
);

/// Params of `portal.devices.register`: `{accountId?, udid, name} → ProvisionPlan (applied per policy)`.
pub type PortalDevicesRegisterParams = Unspecified;
/// Result of `portal.devices.register`.
pub type PortalDevicesRegisterResult = Unspecified;
crate::rpc_method!(
    /// `portal.devices.register` (class `account`). W12 (Wave 2).
    PortalDevicesRegister,
    "portal.devices.register",
    PortalDevicesRegisterParams => PortalDevicesRegisterResult,
    Permission::Account
);

/// Params of `portal.certificates.revoke`: `{accountId?, certId} → {} (typed confirmation; cli/ide only)`.
pub type PortalCertificatesRevokeParams = Unspecified;
/// Result of `portal.certificates.revoke`.
pub type PortalCertificatesRevokeResult = Unspecified;
crate::rpc_method!(
    /// `portal.certificates.revoke` (class `account-destructive`, typed confirmation). W12 (Wave 2).
    PortalCertificatesRevoke,
    "portal.certificates.revoke",
    PortalCertificatesRevokeParams => PortalCertificatesRevokeResult,
    Permission::AccountDestructive, confirm = Typed
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[
    PortalCertificatesList::INFO,
    PortalDevicesList::INFO,
    PortalBundleIdsList::INFO,
    PortalProfilesList::INFO,
    PortalDevicesRegister::INFO,
    PortalCertificatesRevoke::INFO,
];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
