// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `location.*`: simulated location. Owner: W10.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.2 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `location.set`: `{udid?, lat, lon} → {}`.
pub type LocationSetParams = Unspecified;
/// Result of `location.set`.
pub type LocationSetResult = Unspecified;
crate::rpc_method!(
    /// `location.set` (class `device`). W10 (Wave 2).
    LocationSet,
    "location.set",
    LocationSetParams => LocationSetResult,
    Permission::Device
);

/// Params of `location.clear`: `{udid?} → {}`.
pub type LocationClearParams = Unspecified;
/// Result of `location.clear`.
pub type LocationClearResult = Unspecified;
crate::rpc_method!(
    /// `location.clear` (class `device`). W10 (Wave 2).
    LocationClear,
    "location.clear",
    LocationClearParams => LocationClearResult,
    Permission::Device
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[LocationSet::INFO, LocationClear::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
