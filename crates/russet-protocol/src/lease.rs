// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `lease.*`: device reservations, forwards, launches and sessions (ARCHITECTURE §6.3, §8.1). Owner: W2.

use russet_core::ids::LeaseId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::ConnectionRef;
use crate::{ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod};

/// Lease kinds (ARCHITECTURE §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum LeaseKind {
    /// `device.reserve`: exclusive to its principal (the owner plus connections sharing it).
    DeviceReservation,
    /// Internal: the device tunnel, kept warm for `device.keep-warm`.
    Tunnel,
    /// `forward.open`, `flutter.device.forward`.
    Forward,
    /// `app.launch`.
    AppLaunch,
    /// `run.start`, `debug.prepare`, `debug.attach`, `flutter.run`.
    Session,
    /// `screen.mirror.start`.
    Mirror,
    /// `input.*` holds.
    Input,
}

/// One lease (`lease.list`, `daemon.status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LeaseInfo {
    pub lease_id: LeaseId,
    pub kind: LeaseKind,
    /// The connection that holds it; omitted for a detached lease whose owner is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub owner: Option<ConnectionRef>,
    /// Survives its owner (`sessionLease: detached`).
    pub detached: bool,
    /// What it pins: a UDID, a forward, a session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub purpose: Option<String>,
    /// RFC 3339 UTC.
    pub created_at: String,
    /// Connections that presented this lease as `initialize.sharedLease` (device reservations).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_with: Vec<String>,
}

/// Params of `lease.list`: `{}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LeaseListParams {}

/// Result of `lease.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LeaseListResult {
    pub leases: Vec<LeaseInfo>,
}

crate::rpc_method!(
    /// `lease.list` (class `read`). W2 (Wave 1).
    LeaseList,
    "lease.list",
    LeaseListParams => LeaseListResult,
    Permission::Read
);

/// Params of `lease.release`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LeaseReleaseParams {
    pub lease_id: LeaseId,
}

/// Result of `lease.release`: `{}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LeaseReleaseResult {}

crate::rpc_method!(
    /// `lease.release` (class: the owner → `read`, anyone else → `system`; resolved per call, ceiling `system`).
    /// W2 (Wave 1).
    LeaseRelease,
    "lease.release",
    LeaseReleaseParams => LeaseReleaseResult,
    Permission::System, permission_fn = lease_release_permission
);

/// Per-call class of `lease.release` [D 66]: the owner's own lease → `read`, anyone else → `system`. A
/// connection that only shares a reservation is not its owner (ARCHITECTURE §6.3).
pub fn lease_release_permission(
    p: &LeaseReleaseParams,
    c: &ConnInfo,
    o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(if o.owns_lease(&c.id, &p.lease_id) { Permission::Read } else { Permission::System })
}

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[LeaseList::INFO, LeaseRelease::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
