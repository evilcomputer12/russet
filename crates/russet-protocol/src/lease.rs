// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `lease.*`: device reservations, forwards, launches and sessions (ARCHITECTURE §6.3). Owner: W2.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.1 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod, Unspecified};

/// Params of `lease.list`: `{} → {leases: Lease[]}`.
pub type LeaseListParams = Unspecified;
/// Result of `lease.list`.
pub type LeaseListResult = Unspecified;
crate::rpc_method!(
    /// `lease.list` (class `read`). W2 (Wave 1).
    LeaseList,
    "lease.list",
    LeaseListParams => LeaseListResult,
    Permission::Read
);

/// Params of `lease.release`: `{leaseId} → {} (class: owner → read, else system)`.
pub type LeaseReleaseParams = Unspecified;
/// Result of `lease.release`.
pub type LeaseReleaseResult = Unspecified;
crate::rpc_method!(
    /// `lease.release` (class: the owner → `read`, anyone else → `system`; resolved per call, ceiling `system`).
    /// W2 (Wave 1).
    LeaseRelease,
    "lease.release",
    LeaseReleaseParams => LeaseReleaseResult,
    Permission::System, permission_fn = lease_release_permission
);

/// Per-call class of `lease.release` [D 66]: own lease → `read`, else `system`.
///
/// F0 stub: fails closed at the ceiling (`system`) until the owner reads the params.
pub fn lease_release_permission(
    _p: &LeaseReleaseParams,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(Permission::System)
}

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[LeaseList::INFO, LeaseRelease::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
