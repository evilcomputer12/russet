// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `approval.*`: out-of-band approvals (ARCHITECTURE §6.5, §11.3). Owner: W2.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.1 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `approval.list`: `{state?} → {approvals: Approval[]}`.
pub type ApprovalListParams = Unspecified;
/// Result of `approval.list`.
pub type ApprovalListResult = Unspecified;
crate::rpc_method!(
    /// `approval.list` (class `read`). W2 (Wave 1).
    ApprovalList,
    "approval.list",
    ApprovalListParams => ApprovalListResult,
    Permission::Read
);

/// Params of `approval.get`: `{approvalId, waitMs?} → Approval`.
pub type ApprovalGetParams = Unspecified;
/// Result of `approval.get`.
pub type ApprovalGetResult = Unspecified;
crate::rpc_method!(
    /// `approval.get` (class `read`). W2 (Wave 1).
    ApprovalGet,
    "approval.get",
    ApprovalGetParams => ApprovalGetResult,
    Permission::Read
);

/// Params of `approval.grant`: `{approvalId, scope?: once|session} → Approval (kind ide, or cli with interactive)`.
pub type ApprovalGrantParams = Unspecified;
/// Result of `approval.grant`.
pub type ApprovalGrantResult = Unspecified;
crate::rpc_method!(
    /// `approval.grant` (class `system`). W2 (Wave 1).
    ApprovalGrant,
    "approval.grant",
    ApprovalGrantParams => ApprovalGrantResult,
    Permission::System
);

/// Params of `approval.deny`: `{approvalId, reason?} → Approval (same restriction)`.
pub type ApprovalDenyParams = Unspecified;
/// Result of `approval.deny`.
pub type ApprovalDenyResult = Unspecified;
crate::rpc_method!(
    /// `approval.deny` (class `system`). W2 (Wave 1).
    ApprovalDeny,
    "approval.deny",
    ApprovalDenyParams => ApprovalDenyResult,
    Permission::System
);

/// Params of the `approval.requested` notification: `{approval: Approval}; to ide and interactive cli connections`.
pub type ApprovalRequestedParams = Unspecified;
crate::rpc_notification!(
    /// `approval.requested` (daemon → client). W2.
    ApprovalRequested,
    "approval.requested",
    ApprovalRequestedParams
);

/// Params of the `approval.resolved` notification: `{approvalId, state}; the same, plus the requester`.
pub type ApprovalResolvedParams = Unspecified;
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
