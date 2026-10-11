// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `mcp` handlers (§8.6). Owner: W13 (Wave 2), W20 (Wave 3).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::mcp as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::McpStatus, _, _>(not_implemented::<p::McpStatus>);
    r.request::<p::McpInstall, _, _>(not_implemented::<p::McpInstall>);
    r.request::<p::McpUninstall, _, _>(not_implemented::<p::McpUninstall>);
    r.request::<p::McpHttpSet, _, _>(not_implemented::<p::McpHttpSet>);
    r.request::<p::McpPolicyGet, _, _>(not_implemented::<p::McpPolicyGet>);
    r.request::<p::McpPolicySet, _, _>(not_implemented::<p::McpPolicySet>);
}
