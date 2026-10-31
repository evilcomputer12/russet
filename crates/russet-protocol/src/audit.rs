// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `audit.*`: the append-only `state/audit.jsonl` (ARCHITECTURE §6.5, §8.1). Owner: W2.
//!
//! Every MCP call is recorded, and CLI and IDE calls from class `account` upwards, with redacted params.

use russet_core::ids::ApprovalId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::ClientRef;
use crate::{MethodInfo, NotificationInfo, RpcMethod};

/// What an audit entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AuditKind {
    /// A call (or a refused call).
    Rpc,
    /// An approval was requested, granted, denied or used.
    Approval,
    /// A change on Apple's side (`account`, `account-destructive`, `publish`).
    AccountMutation,
}

/// One line of `state/audit.jsonl` (ARCHITECTURE §6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    /// RFC 3339 UTC.
    pub ts: String,
    pub kind: AuditKind,
    pub client: ClientRef,
    pub method: String,
    /// The MCP tool behind the call, when the bridge named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tool: Option<String>,
    /// The MCP tier of the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tier: Option<u8>,
    /// SHA-256 of the canonical JSON params without `approval`.
    pub params_digest: String,
    /// The params with secrets masked (ARCHITECTURE §12.2).
    pub params_redacted: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_id: Option<ApprovalId>,
    /// `ok`, or the `russetCode` of the error (`perm.denied`, …), or an approval state for `approval` entries.
    pub outcome: String,
    pub duration_ms: u64,
}

/// Params of `audit.tail`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AuditTailParams {
    /// The newest entries to return; default 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<AuditKind>,
    /// Only this client name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub client: Option<String>,
}

/// Result of `audit.tail`: oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AuditTailResult {
    pub entries: Vec<AuditEntry>,
}

crate::rpc_method!(
    /// `audit.tail` (class `read`). W2 (Wave 1).
    AuditTail,
    "audit.tail",
    AuditTailParams => AuditTailResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[AuditTail::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
