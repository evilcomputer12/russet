// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `logs.*`: device logs (`LogEntry`, ARCHITECTURE §9.5). Owner: W4.

use russet_core::ids::SubscriptionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{LogEntry, LogLevel};
use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification};

/// A device log source a subscription reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum LogsSource {
    /// `os_trace_relay`, structured, filtered by pid on the device.
    Oslog,
    /// `syslog_relay` text lines.
    Syslog,
    /// The console of an app Russet launched.
    Stdout,
}

/// Which entries a subscription delivers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LogFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pid: Option<u32>,
    /// Resolved to the app's running process when the subscription starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub process: Option<String>,
    /// A subsystem prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub subsystem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub min_level: Option<LogLevel>,
    /// A case-insensitive substring of the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub contains: Option<String>,
}

/// Params of `logs.subscribe`: `{udid?, sources: (oslog|syslog|stdout)[], filter: LogFilter, backlog?} → {subscriptionId} ⇒ logs.entries`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LogsSubscribeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// Empty means `oslog` and `stdout`.
    #[serde(default)]
    pub sources: Vec<LogsSource>,
    #[serde(default)]
    pub filter: LogFilter,
    /// Entries from before the subscription to deliver first (from the console backlog).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub backlog: Option<u32>,
}

/// Result of `logs.subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LogsSubscribeResult {
    pub subscription_id: SubscriptionId,
}
crate::rpc_method!(
    /// `logs.subscribe` (class `read`). W4 (Wave 1).
    LogsSubscribe,
    "logs.subscribe",
    LogsSubscribeParams => LogsSubscribeResult,
    Permission::Read
);

/// Params of `logs.tail`: `{udid?, bundleId?, pid?, lines?, minLevel?, contains?, since?, cursor?} → {entries: LogEntry[], truncated, cursor?}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LogsTailParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pid: Option<u32>,
    /// Default 200, at most 2000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub lines: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub min_level: Option<LogLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub contains: Option<String>,
    /// RFC 3339 UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub since: Option<String>,
    /// Only entries after a previous result's cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cursor: Option<String>,
}

/// Result of `logs.tail`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LogsTailResult {
    pub entries: Vec<LogEntry>,
    /// More entries matched than `lines`.
    pub truncated: bool,
    /// Pass it back as `cursor` to get only newer entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cursor: Option<String>,
}
crate::rpc_method!(
    /// `logs.tail` (class `read`). W4 (Wave 1).
    LogsTail,
    "logs.tail",
    LogsTailParams => LogsTailResult,
    Permission::Read
);

/// Params of the `logs.entries` notification: `{subscriptionId, entries: LogEntry[], dropped?}; to subscribers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LogsEntriesParams {
    pub subscription_id: SubscriptionId,
    pub entries: Vec<LogEntry>,
    /// Entries lost since the previous notification (slow consumer, ARCHITECTURE §6.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub dropped: Option<u64>,
}
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
