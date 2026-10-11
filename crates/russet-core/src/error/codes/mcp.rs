// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `mcp` error codes (9000–9099). Owner: W13.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! ARCHITECTURE §13.4 lists no exit code for this family; W1 uses 1 (unknown) for `mcp.toolNotFound` and 11 (permission) for the HTTP authentication failures, like `perm.authFailed`.

use crate::error::ErrorCode;

/// 9000 `mcp.toolNotFound`: No MCP tool with this name.
pub const TOOL_NOT_FOUND: ErrorCode = ErrorCode::new(9000, "mcp.toolNotFound", 1);
/// 9001 `mcp.httpUnauthorized`: The MCP HTTP request has no valid bearer token.
pub const HTTP_UNAUTHORIZED: ErrorCode = ErrorCode::new(9001, "mcp.httpUnauthorized", 11);
/// 9002 `mcp.originRejected`: The MCP HTTP request's Origin or Host was rejected.
pub const ORIGIN_REJECTED: ErrorCode = ErrorCode::new(9002, "mcp.originRejected", 11);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[TOOL_NOT_FOUND, HTTP_UNAUTHORIZED, ORIGIN_REJECTED];
