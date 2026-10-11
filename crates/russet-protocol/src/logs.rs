// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `logs.*`: device logs (`LogEntry`, ARCHITECTURE §9.5). Owner: W4.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.2 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `logs.subscribe`: `{udid?, sources: (oslog|syslog|stdout)[], filter: LogFilter, backlog?} → {subscriptionId} ⇒ logs.entries`.
pub type LogsSubscribeParams = Unspecified;
/// Result of `logs.subscribe`.
pub type LogsSubscribeResult = Unspecified;
crate::rpc_method!(
    /// `logs.subscribe` (class `read`). W4 (Wave 1).
    LogsSubscribe,
    "logs.subscribe",
    LogsSubscribeParams => LogsSubscribeResult,
    Permission::Read
);

/// Params of `logs.tail`: `{udid?, bundleId?, pid?, lines?, minLevel?, contains?, since?, cursor?} → {entries: LogEntry[], truncated, cursor?}`.
pub type LogsTailParams = Unspecified;
/// Result of `logs.tail`.
pub type LogsTailResult = Unspecified;
crate::rpc_method!(
    /// `logs.tail` (class `read`). W4 (Wave 1).
    LogsTail,
    "logs.tail",
    LogsTailParams => LogsTailResult,
    Permission::Read
);

/// Params of the `logs.entries` notification: `{subscriptionId, entries: LogEntry[], dropped?}; to subscribers`.
pub type LogsEntriesParams = Unspecified;
crate::rpc_notification!(
    /// `logs.entries` (daemon → client). W4.
    LogsEntries,
    "logs.entries",
    LogsEntriesParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[LogsSubscribe::INFO, LogsTail::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[LogsEntries::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
