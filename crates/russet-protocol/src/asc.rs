// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `asc.*`: allowlisted App Store Connect GETs. Owner: W12.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.4 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `asc.get`: `{pathAndQuery} → {status, body} (allowlisted GET; request-scoped token)`.
pub type AscGetParams = Unspecified;
/// Result of `asc.get`.
pub type AscGetResult = Unspecified;
crate::rpc_method!(
    /// `asc.get` (class `read`). W12 (Wave 2).
    AscGet,
    "asc.get",
    AscGetParams => AscGetResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[AscGet::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
