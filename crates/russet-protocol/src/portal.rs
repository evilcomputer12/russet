// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `portal.*`: developer-portal records (UDIDs masked unless revealed to ide). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.4 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `portal.certificates.list`: `{accountId?, filter?} → records`.
pub type PortalCertificatesListParams = Unspecified;
/// Result of `portal.certificates.list`.
pub type PortalCertificatesListResult = Unspecified;
crate::rpc_method!(
    /// `portal.certificates.list` (class `read`). W6 (Wave 1).
    PortalCertificatesList,
    "portal.certificates.list",
    PortalCertificatesListParams => PortalCertificatesListResult,
    Permission::Read
);

/// Params of `portal.devices.list`: `{accountId?, filter?} → records`.
pub type PortalDevicesListParams = Unspecified;
/// Result of `portal.devices.list`.
pub type PortalDevicesListResult = Unspecified;
crate::rpc_method!(
    /// `portal.devices.list` (class `read`). W6 (Wave 1).
    PortalDevicesList,
    "portal.devices.list",
    PortalDevicesListParams => PortalDevicesListResult,
    Permission::Read
);

/// Params of `portal.bundleIds.list`: `{accountId?, filter?} → records`.
pub type PortalBundleIdsListParams = Unspecified;
/// Result of `portal.bundleIds.list`.
pub type PortalBundleIdsListResult = Unspecified;
crate::rpc_method!(
    /// `portal.bundleIds.list` (class `read`). W6 (Wave 1).
    PortalBundleIdsList,
    "portal.bundleIds.list",
    PortalBundleIdsListParams => PortalBundleIdsListResult,
    Permission::Read
);

/// Params of `portal.profiles.list`: `{accountId?, filter?} → records`.
pub type PortalProfilesListParams = Unspecified;
/// Result of `portal.profiles.list`.
pub type PortalProfilesListResult = Unspecified;
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
