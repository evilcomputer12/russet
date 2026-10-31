// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `daemon.*`: daemon status, shutdown and logs (ARCHITECTURE §8.1, §9.1). Owner: W2.

use std::collections::BTreeMap;

use russet_core::ids::SubscriptionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::lease::LeaseInfo;
use crate::{ClientKind, MethodInfo, NotificationInfo, RpcMethod, RpcNotification};

/// Params of `daemon.ping`: `{}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DaemonPingParams {}
/// Result of `daemon.ping`: `{}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DaemonPingResult {}
crate::rpc_method!(
    /// `daemon.ping` (class `none`). W2 (Wave 1).
    DaemonPing,
    "daemon.ping",
    DaemonPingParams => DaemonPingResult,
    Permission::None
);

/// Params of `daemon.status`: `{}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DaemonStatusParams {}

/// Result of `daemon.status` (ARCHITECTURE §8.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatusResult {
    pub version: String,
    pub build_id: String,
    pub pid: u32,
    pub uptime_ms: u64,
    /// `unix:/…/russetd.sock`, `pipe:\\.\pipe\…`, `tcp://127.0.0.1:<port>` or `inproc`.
    pub endpoint: String,
    /// The web endpoint, once started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub web: Option<String>,
    pub connections: Vec<ConnectionSummary>,
    pub jobs: JobCounts,
    pub leases: Vec<LeaseInfo>,
    /// Devices the device plane knows (0 until it starts, ARCHITECTURE §3.3 item 7).
    pub devices: u32,
    /// The daemon's resident set size (0 where the platform does not report it).
    pub rss_bytes: u64,
    /// RFC 3339 UTC.
    pub started_at: String,
    /// The device backend (`live`, `scripted:<file>`, …, `none`).
    pub device_backend: String,
}

/// One connection in `daemon.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSummary {
    /// `c-<n>`.
    pub id: String,
    pub kind: ClientKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pid: Option<u32>,
}

/// Job counts in `daemon.status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobCounts {
    pub queued: u32,
    pub running: u32,
}

crate::rpc_method!(
    /// `daemon.status` (class `read`). W2 (Wave 1).
    DaemonStatus,
    "daemon.status",
    DaemonStatusParams => DaemonStatusResult,
    Permission::Read
);

/// Why the daemon stops (`daemon.shutdown`, `daemon.shuttingDown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ShutdownReason {
    Idle,
    Request,
    Upgrade,
    Signal,
}

/// Params of `daemon.shutdown`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonShutdownParams {
    /// Stop even with active jobs and leases, and skip the 10 s job grace.
    #[serde(default)]
    pub force: bool,
    /// Default `request`. With `upgrade` the daemon accepts only when it is idle apart from the requester
    /// (ARCHITECTURE §3.3 item 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<ShutdownReason>,
}

/// Result of `daemon.shutdown`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonShutdownResult {
    pub accepted: bool,
    /// Running or queued jobs plus leases that kept (or would have kept) the daemon alive.
    pub active_sessions: u32,
}

crate::rpc_method!(
    /// `daemon.shutdown` (class `system`). W2 (Wave 1).
    DaemonShutdown,
    "daemon.shutdown",
    DaemonShutdownParams => DaemonShutdownResult,
    Permission::System
);

/// Params of `daemon.setLogFilter`: an `EnvFilter` directive such as `info,russetd=debug`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonSetLogFilterParams {
    pub filter: String,
}

/// Result of `daemon.setLogFilter`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonSetLogFilterResult {
    pub previous: String,
}

crate::rpc_method!(
    /// `daemon.setLogFilter` (class `system`). W2 (Wave 1).
    DaemonSetLogFilter,
    "daemon.setLogFilter",
    DaemonSetLogFilterParams => DaemonSetLogFilterResult,
    Permission::System
);

/// The daemon's own log levels (tracing's, not Apple's `LogLevel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum TraceLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Params of `daemon.logs.subscribe`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonLogsSubscribeParams {
    /// Default `info`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub min_level: Option<TraceLevel>,
}

/// Result of `daemon.logs.subscribe`; records follow as `daemon.logBatch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonLogsSubscribeResult {
    pub subscription_id: SubscriptionId,
}

crate::rpc_method!(
    /// `daemon.logs.subscribe` (class `read`). W2 (Wave 1).
    DaemonLogsSubscribe,
    "daemon.logs.subscribe",
    DaemonLogsSubscribeParams => DaemonLogsSubscribeResult,
    Permission::Read
);

/// Params of the `daemon.shuttingDown` notification (to all connections).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonShuttingDownParams {
    pub reason: ShutdownReason,
    /// How long jobs get before their process trees are killed.
    pub grace_ms: u64,
}
crate::rpc_notification!(
    /// `daemon.shuttingDown` (daemon → client). W2.
    DaemonShuttingDown,
    "daemon.shuttingDown",
    DaemonShuttingDownParams
);

/// One daemon log record (already redacted, ARCHITECTURE §12.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonLogRecord {
    /// RFC 3339 UTC.
    pub ts: String,
    pub level: TraceLevel,
    pub target: String,
    pub message: String,
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

/// Params of the `daemon.logBatch` notification (to subscribers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonLogBatchParams {
    pub subscription_id: SubscriptionId,
    pub records: Vec<DaemonLogRecord>,
    /// Records lost to a slow consumer since the previous batch (ARCHITECTURE §6.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dropped: Option<u64>,
}
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn status_has_the_fields_of_section_8_1() {
        let s = DaemonStatusResult {
            version: "0.1.0".into(),
            build_id: "0.1.0+g1a2b3c4".into(),
            pid: 4242,
            uptime_ms: 5,
            endpoint: "unix:/run/user/1000/russet/russetd.sock".into(),
            web: None,
            connections: vec![ConnectionSummary {
                id: "c-1".into(),
                kind: ClientKind::Cli,
                name: "russet".into(),
                pid: Some(7),
            }],
            jobs: JobCounts { queued: 0, running: 1 },
            leases: vec![],
            devices: 0,
            rss_bytes: 1024,
            started_at: "2026-10-11T09:00:00.000Z".into(),
            device_backend: "none".into(),
        };
        let v = serde_json::to_value(&s).expect("serializes");
        for key in [
            "version",
            "buildId",
            "pid",
            "uptimeMs",
            "endpoint",
            "connections",
            "jobs",
            "leases",
            "devices",
            "rssBytes",
        ] {
            assert!(v.get(key).is_some(), "{key} missing in {v}");
        }
        assert!(v.get("web").is_none(), "optional fields are omitted");
        assert_eq!(v["connections"][0], json!({"id": "c-1", "kind": "cli", "name": "russet", "pid": 7}));
    }

    #[test]
    fn shutdown_defaults() {
        let p: DaemonShutdownParams = serde_json::from_value(json!({})).expect("parses");
        assert_eq!(p, DaemonShutdownParams { force: false, reason: None });
        let p: DaemonShutdownParams = serde_json::from_value(json!({"reason": "upgrade"})).expect("parses");
        assert_eq!(p.reason, Some(ShutdownReason::Upgrade));
    }
}
