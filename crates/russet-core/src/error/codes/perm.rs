// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `perm` error codes (1200–1299). Owner: W2.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 11 (permission denied, declined, or approval needed).

use crate::error::ErrorCode;

/// 1200 `perm.denied`: The connection's grant does not allow this method.
pub const DENIED: ErrorCode = ErrorCode::new(1200, "perm.denied", 11);
/// 1201 `perm.confirmationDeclined`: The user declined the confirmation.
pub const CONFIRMATION_DECLINED: ErrorCode = ErrorCode::new(1201, "perm.confirmationDeclined", 11);
/// 1202 `perm.confirmationUnavailable`: A confirmation is needed but there is no TTY and no `--yes`.
pub const CONFIRMATION_UNAVAILABLE: ErrorCode = ErrorCode::new(1202, "perm.confirmationUnavailable", 11);
/// 1203 `perm.authFailed`: Authentication failed (peer uid/SID, TCP token).
pub const AUTH_FAILED: ErrorCode = ErrorCode::new(1203, "perm.authFailed", 11);
/// 1204 `perm.approvalRequired`: An out-of-band approval is required; `details {approvalId, tool?, tier, summary, argsDigest, approveWith, expiresAt}` (ARCHITECTURE §11.3). Was RUS-MCP-0403.
pub const APPROVAL_REQUIRED: ErrorCode = ErrorCode::new(1204, "perm.approvalRequired", 11);
/// 1205 `perm.approvalInvalid`: The approval is expired, used, or bound to other arguments.
pub const APPROVAL_INVALID: ErrorCode = ErrorCode::new(1205, "perm.approvalInvalid", 11);
/// 1206 `perm.rateLimited`: The tool's rate limit was hit.
pub const RATE_LIMITED: ErrorCode = ErrorCode::new(1206, "perm.rateLimited", 11);
/// 1207 `perm.tierExceeded`: The method's MCP tier is above the client's `tier-max`.
pub const TIER_EXCEEDED: ErrorCode = ErrorCode::new(1207, "perm.tierExceeded", 11);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    DENIED,
    CONFIRMATION_DECLINED,
    CONFIRMATION_UNAVAILABLE,
    AUTH_FAILED,
    APPROVAL_REQUIRED,
    APPROVAL_INVALID,
    RATE_LIMITED,
    TIER_EXCEEDED,
];

/// Default hints (ARCHITECTURE §13.1), used when an error of this family carries none. A declined confirmation
/// has no next step beyond running the command again.
pub const HINTS: &[(ErrorCode, &str)] = &[
    (DENIED, "This client's permission profile does not allow the call (ARCHITECTURE §7.5)."),
    (CONFIRMATION_UNAVAILABLE, "Run it in a terminal to confirm, or pass `--yes` (typed ones need `--confirm-text`)."),
    (AUTH_FAILED, "The connection was not authenticated: check the token or the socket's owner."),
    (APPROVAL_REQUIRED, "A human must approve it (`russet approve <id>` in a terminal, or Approve in Russet IDE)."),
    (APPROVAL_INVALID, "Request a new approval: retry the call without `approval`."),
    (RATE_LIMITED, "Wait and retry; the limits are set in mcp-policy.toml."),
    (TIER_EXCEEDED, "Raise the client's `tier-max` in mcp-policy.toml if a human agrees (ARCHITECTURE §11.3)."),
];
