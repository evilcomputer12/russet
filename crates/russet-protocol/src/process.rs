// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `process.*`: process list and signals (ARCHITECTURE §8.2). Owner: W4.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod};

/// Params of `process.list`: `{udid?} → {processes: ProcessInfo[]}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProcessListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
}

/// One process on the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    /// The executable's file name.
    pub name: String,
    /// The executable's device path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub executable: Option<String>,
    /// For user apps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
    /// Runs from an `.app` bundle.
    pub is_application: bool,
}

/// Result of `process.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProcessListResult {
    pub processes: Vec<ProcessInfo>,
}
crate::rpc_method!(
    /// `process.list` (class `read`). W4 (Wave 1).
    ProcessList,
    "process.list",
    ProcessListParams => ProcessListResult,
    Permission::Read
);

/// Params of `process.signal`: `{udid?, pid, signal} → {}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSignalParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    pub pid: u32,
    /// A POSIX signal number, for example 19 (SIGCONT) or 9 (SIGKILL).
    pub signal: i32,
}

/// Result of `process.signal`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSignalResult {}
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
