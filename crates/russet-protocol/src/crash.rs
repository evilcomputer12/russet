// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `crash.*`: the local crash store and symbolication. Owner: W11.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.5 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `crash.list`: `{bundleId?, since?, limit?} → {crashes: CrashSummary[]} (local store)`.
pub type CrashListParams = Unspecified;
/// Result of `crash.list`.
pub type CrashListResult = Unspecified;
crate::rpc_method!(
    /// `crash.list` (class `read`). W11 (Wave 2).
    CrashList,
    "crash.list",
    CrashListParams => CrashListResult,
    Permission::Read
);

/// Params of `crash.fetch`: `{udid?, bundleId?, since?} → {fetched: CrashSummary[]}`.
pub type CrashFetchParams = Unspecified;
/// Result of `crash.fetch`.
pub type CrashFetchResult = Unspecified;
crate::rpc_method!(
    /// `crash.fetch` (class `device`, long-running). W11 (Wave 2).
    CrashFetch,
    "crash.fetch",
    CrashFetchParams => CrashFetchResult,
    Permission::Device, long_running
);

/// Params of `crash.symbolicate`: `{crashId?|path?, format?: json|text} → {crash}`.
pub type CrashSymbolicateParams = Unspecified;
/// Result of `crash.symbolicate`.
pub type CrashSymbolicateResult = Unspecified;
crate::rpc_method!(
    /// `crash.symbolicate` (class `read`). W11 (Wave 2).
    CrashSymbolicate,
    "crash.symbolicate",
    CrashSymbolicateParams => CrashSymbolicateResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[CrashList::INFO, CrashFetch::INFO, CrashSymbolicate::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
