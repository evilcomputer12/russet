// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `doctor.*`: environment checks (ARCHITECTURE §8.1; area checks register through russetd's doctor seam). Owner: W3.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.1 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod, Unspecified};

/// Params of `doctor.run`: `{checks?, categories?, project?, udid?, online?, fix?} → DoctorReport (class: read, fix → system)`.
pub type DoctorRunParams = Unspecified;
/// Result of `doctor.run`.
pub type DoctorRunResult = Unspecified;
crate::rpc_method!(
    /// `doctor.run` (class `system`). W3 (+ area checks) (Wave 1).
    DoctorRun,
    "doctor.run",
    DoctorRunParams => DoctorRunResult,
    Permission::System, permission_fn = doctor_run_permission
);

/// Per-call class of `doctor.run` [D 66]: `read`, or `system` with `fix`.
///
/// F0 stub: fails closed at the ceiling (`system`) until the owner reads the params.
pub fn doctor_run_permission(
    _p: &DoctorRunParams,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(Permission::System)
}

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[DoctorRun::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
