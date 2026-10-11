// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `daemon.*`: daemon status, shutdown and logs. Owner: W2.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.1 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `daemon.ping`: `{} → {}`.
pub type DaemonPingParams = Unspecified;
/// Result of `daemon.ping`.
pub type DaemonPingResult = Unspecified;
crate::rpc_method!(
    /// `daemon.ping` (class `none`). W2 (Wave 1).
    DaemonPing,
    "daemon.ping",
    DaemonPingParams => DaemonPingResult,
    Permission::None
);

/// Params of `daemon.status`: `{} → {version, buildId, pid, uptimeMs, endpoint, web?, connections[{id,kind,name,pid}], jobs{queued,running}, leases[], devices, rssBytes}`.
pub type DaemonStatusParams = Unspecified;
/// Result of `daemon.status`.
pub type DaemonStatusResult = Unspecified;
crate::rpc_method!(
    /// `daemon.status` (class `read`). W2 (Wave 1).
    DaemonStatus,
    "daemon.status",
    DaemonStatusParams => DaemonStatusResult,
    Permission::Read
);

/// Params of `daemon.shutdown`: `{force?, reason?} → {accepted, activeSessions}`.
pub type DaemonShutdownParams = Unspecified;
/// Result of `daemon.shutdown`.
pub type DaemonShutdownResult = Unspecified;
crate::rpc_method!(
    /// `daemon.shutdown` (class `system`). W2 (Wave 1).
    DaemonShutdown,
    "daemon.shutdown",
    DaemonShutdownParams => DaemonShutdownResult,
    Permission::System
);

/// Params of `daemon.setLogFilter`: `{filter} → {previous}`.
pub type DaemonSetLogFilterParams = Unspecified;
/// Result of `daemon.setLogFilter`.
pub type DaemonSetLogFilterResult = Unspecified;
crate::rpc_method!(
    /// `daemon.setLogFilter` (class `system`). W2 (Wave 1).
    DaemonSetLogFilter,
    "daemon.setLogFilter",
    DaemonSetLogFilterParams => DaemonSetLogFilterResult,
    Permission::System
);

/// Params of `daemon.logs.subscribe`: `{minLevel?} → {subscriptionId} ⇒ daemon.logBatch`.
pub type DaemonLogsSubscribeParams = Unspecified;
/// Result of `daemon.logs.subscribe`.
pub type DaemonLogsSubscribeResult = Unspecified;
crate::rpc_method!(
    /// `daemon.logs.subscribe` (class `read`). W2 (Wave 1).
    DaemonLogsSubscribe,
    "daemon.logs.subscribe",
    DaemonLogsSubscribeParams => DaemonLogsSubscribeResult,
    Permission::Read
);

/// Params of the `daemon.shuttingDown` notification: `{reason: idle|request|upgrade|signal, graceMs}; to all`.
pub type DaemonShuttingDownParams = Unspecified;
crate::rpc_notification!(
    /// `daemon.shuttingDown` (daemon → client). W2.
    DaemonShuttingDown,
    "daemon.shuttingDown",
    DaemonShuttingDownParams
);

/// Params of the `daemon.logBatch` notification: `{subscriptionId, records[{ts, level, target, message, fields}], dropped?}; to subscribers`.
pub type DaemonLogBatchParams = Unspecified;
crate::rpc_notification!(
    /// `daemon.logBatch` (daemon → client). W2.
    DaemonLogBatch,
    "daemon.logBatch",
    DaemonLogBatchParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] =
    &[DaemonPing::INFO, DaemonStatus::INFO, DaemonShutdown::INFO, DaemonSetLogFilter::INFO, DaemonLogsSubscribe::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[DaemonShuttingDown::INFO, DaemonLogBatch::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
