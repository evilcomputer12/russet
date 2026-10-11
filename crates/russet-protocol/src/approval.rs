// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `approval.*`: out-of-band approvals (ARCHITECTURE §6.5, §8.1, §11.3). Owner: W2.
//!
//! A tier-2 or tier-3 call from an MCP client without a valid approval fails with `perm.approvalRequired`
//! ([`ApprovalRequiredDetails`]); the daemon announces the pending [`Approval`] with `approval.requested`; a
//! human grants it from Russet IDE or `russet approve` in a TTY; the agent retries with `approval: "apr_…"`.
//! Confirmations of `cli` and `ide` calls are recorded as approvals too, granted by the requester.

use russet_core::ids::ApprovalId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::ConnectionRef;
use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification};

/// Approval states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalState {
    Pending,
    Granted,
    Denied,
    Expired,
    Used,
}

impl ApprovalState {
    /// Whether the approval can no longer change.
    pub fn is_final(self) -> bool {
        matches!(self, Self::Denied | Self::Expired | Self::Used)
    }
}

/// How long a grant lasts (ARCHITECTURE §11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalScope {
    /// One call (tier 3, and the default).
    Once,
    /// Every call of the method from the requesting MCP session (tier 2) [D 67].
    Session,
}

/// What an approval allows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum ApprovalSubject {
    /// One method call; `paramsDigest` is the SHA-256 of the canonical JSON params without `approval`.
    Method { method: String, params_digest: String },
    /// A provisioning plan (`provision.apply`, W12).
    Plan { plan_id: String, plan_digest: String },
}

/// An approval record (ARCHITECTURE §6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub approval_id: ApprovalId,
    pub state: ApprovalState,
    pub requested_by: ConnectionRef,
    pub subject: ApprovalSubject,
    /// The MCP tier (2 or 3; 0 for a recorded confirmation).
    pub tier: u8,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scope: Option<ApprovalScope>,
    /// RFC 3339 UTC.
    pub created_at: String,
    /// RFC 3339 UTC: pending approvals expire 10 min after creation.
    pub expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub granted_by: Option<ConnectionRef>,
    /// Why it was denied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
}

/// `details` of `perm.approvalRequired` (1204, ARCHITECTURE §11.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequiredDetails {
    pub approval_id: ApprovalId,
    /// The MCP tool, when the bridge named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tool: Option<String>,
    pub tier: u8,
    pub summary: String,
    pub args_digest: String,
    /// `russet approve apr_… (or Approve in Russet IDE)`.
    pub approve_with: String,
    /// RFC 3339 UTC.
    pub expires_at: String,
}

/// Params of `approval.list`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub state: Option<ApprovalState>,
}

/// Result of `approval.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalListResult {
    pub approvals: Vec<Approval>,
}

crate::rpc_method!(
    /// `approval.list` (class `read`). W2 (Wave 1).
    ApprovalList,
    "approval.list",
    ApprovalListParams => ApprovalListResult,
    Permission::Read
);

/// Params of `approval.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalGetParams {
    pub approval_id: ApprovalId,
    /// Wait up to this long while the approval is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wait_ms: Option<u64>,
}

/// Result of `approval.get`.
pub type ApprovalGetResult = Approval;

crate::rpc_method!(
    /// `approval.get` (class `read`). W2 (Wave 1).
    ApprovalGet,
    "approval.get",
    ApprovalGetParams => ApprovalGetResult,
    Permission::Read
);

/// Params of `approval.grant` (`ide` connections, or `cli` with `interactive`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalGrantParams {
    pub approval_id: ApprovalId,
    /// Default: `session` for tier 2, `once` for tier 3 (which never allows `session`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scope: Option<ApprovalScope>,
}

/// Result of `approval.grant`.
pub type ApprovalGrantResult = Approval;

crate::rpc_method!(
    /// `approval.grant` (class `system`; only `ide`, or `cli` with `interactive`). W2 (Wave 1).
    ApprovalGrant,
    "approval.grant",
    ApprovalGrantParams => ApprovalGrantResult,
    Permission::System
);

/// Params of `approval.deny` (same restriction as `approval.grant`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDenyParams {
    pub approval_id: ApprovalId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
}

/// Result of `approval.deny`.
pub type ApprovalDenyResult = Approval;

crate::rpc_method!(
    /// `approval.deny` (class `system`; only `ide`, or `cli` with `interactive`). W2 (Wave 1).
    ApprovalDeny,
    "approval.deny",
    ApprovalDenyParams => ApprovalDenyResult,
    Permission::System
);

/// Params of the `approval.requested` notification (to `ide` and interactive `cli` connections).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestedParams {
    pub approval: Approval,
}
crate::rpc_notification!(
    /// `approval.requested` (daemon → client). W2.
    ApprovalRequested,
    "approval.requested",
    ApprovalRequestedParams
);

/// Params of the `approval.resolved` notification (the same, plus the requester).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalResolvedParams {
    pub approval_id: ApprovalId,
    pub state: ApprovalState,
}
crate::rpc_notification!(
    /// `approval.resolved` (daemon → client). W2.
    ApprovalResolved,
    "approval.resolved",
    ApprovalResolvedParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[ApprovalList::INFO, ApprovalGet::INFO, ApprovalGrant::INFO, ApprovalDeny::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[ApprovalRequested::INFO, ApprovalResolved::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ClientKind;

    #[test]
    fn approvals_match_section_6_5() {
        let a = Approval {
            approval_id: ApprovalId::parse("apr_01J00000000000000000000000").expect("id"),
            state: ApprovalState::Pending,
            requested_by: ConnectionRef {
                kind: ClientKind::Mcp,
                name: "claude-code".into(),
                connection_id: "c-7".into(),
            },
            subject: ApprovalSubject::Method { method: "ship.upload".into(), params_digest: "ab".into() },
            tier: 3,
            summary: "Upload".into(),
            detail: None,
            scope: None,
            created_at: "t0".into(),
            expires_at: "t1".into(),
            granted_by: None,
            reason: None,
        };
        let v = serde_json::to_value(&a).expect("serializes");
        assert_eq!(v["subject"], json!({"kind": "method", "method": "ship.upload", "paramsDigest": "ab"}));
        assert_eq!(v["requestedBy"], json!({"kind": "mcp", "name": "claude-code", "connectionId": "c-7"}));
        assert_eq!(serde_json::from_value::<Approval>(v).expect("parses"), a);
        let plan: ApprovalSubject =
            serde_json::from_value(json!({"kind": "plan", "planId": "pln_1", "planDigest": "cd"})).expect("parses");
        assert!(matches!(plan, ApprovalSubject::Plan { .. }));
    }
}
