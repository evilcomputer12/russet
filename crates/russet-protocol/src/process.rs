// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `process.*`: process list and signals. Owner: W4.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.2 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `process.list`: `{udid?} → {processes: ProcessInfo[]}`.
pub type ProcessListParams = Unspecified;
/// Result of `process.list`.
pub type ProcessListResult = Unspecified;
crate::rpc_method!(
    /// `process.list` (class `read`). W4 (Wave 1).
    ProcessList,
    "process.list",
    ProcessListParams => ProcessListResult,
    Permission::Read
);

/// Params of `process.signal`: `{udid?, pid, signal} → {}`.
pub type ProcessSignalParams = Unspecified;
/// Result of `process.signal`.
pub type ProcessSignalResult = Unspecified;
crate::rpc_method!(
    /// `process.signal` (class `device`). W4 (Wave 1).
    ProcessSignal,
    "process.signal",
    ProcessSignalParams => ProcessSignalResult,
    Permission::Device
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[ProcessList::INFO, ProcessSignal::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
