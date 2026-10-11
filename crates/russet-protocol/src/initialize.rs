// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `initialize` handshake (ARCHITECTURE §7.4). Owner: W2.
//!
//! F0 stub (W1): every name of ARCHITECTURE §7.4 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `initialize`: `{protocol, client{kind, name, version, pid}, capabilities, workspace?, auth?, sharedLease?} → {protocol, server, connectionId, grant, capabilities, endpoints} (§7.4)`.
pub type InitializeParams = Unspecified;
/// Result of `initialize`.
pub type InitializeResult = Unspecified;
crate::rpc_method!(
    /// `initialize` (class `none`). W2 (Wave 1).
    Initialize,
    "initialize",
    InitializeParams => InitializeResult,
    Permission::None
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[Initialize::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
