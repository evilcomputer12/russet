// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `perf.*`: performance samples. Owner: W10.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.2 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `perf.subscribe`: `{udid?, pid?|bundleId?, intervalMs?, metrics?: (cpu|memory|energy|network|fps)[]} → {subscriptionId} ⇒ perf.samples`.
pub type PerfSubscribeParams = Unspecified;
/// Result of `perf.subscribe`.
pub type PerfSubscribeResult = Unspecified;
crate::rpc_method!(
    /// `perf.subscribe` (class `read`). W10 (Wave 2).
    PerfSubscribe,
    "perf.subscribe",
    PerfSubscribeParams => PerfSubscribeResult,
    Permission::Read
);

/// Params of `perf.export`: `{subscriptionId, path, format: chrome-trace|jsonl|csv} → {path}`.
pub type PerfExportParams = Unspecified;
/// Result of `perf.export`.
pub type PerfExportResult = Unspecified;
crate::rpc_method!(
    /// `perf.export` (class `read`). W10 (Wave 3).
    PerfExport,
    "perf.export",
    PerfExportParams => PerfExportResult,
    Permission::Read
);

/// Params of the `perf.samples` notification: `{subscriptionId, samples[]}; to subscribers`.
pub type PerfSamplesParams = Unspecified;
crate::rpc_notification!(
    /// `perf.samples` (daemon → client). W10.
    PerfSamples,
    "perf.samples",
    PerfSamplesParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[PerfSubscribe::INFO, PerfExport::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[PerfSamples::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
