// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `mcp.*`: MCP administration (ARCHITECTURE §8.6). Owner: W13 (Wave 2), W20 (Wave 3).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.6 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `mcp.status`: `{} → {stdio{command,args}, http?{url, enabled, listeners[]}, clients[{id, registered, scope, path}]}`.
pub type McpStatusParams = Unspecified;
/// Result of `mcp.status`.
pub type McpStatusResult = Unspecified;
crate::rpc_method!(
    /// `mcp.status` (class `read`). W13 (Wave 2).
    McpStatus,
    "mcp.status",
    McpStatusParams => McpStatusResult,
    Permission::Read
);

/// Params of `mcp.install`: `{client, scope?: user|project, project?, dryRun?, tierMax?} → {changes[]}`.
pub type McpInstallParams = Unspecified;
/// Result of `mcp.install`.
pub type McpInstallResult = Unspecified;
crate::rpc_method!(
    /// `mcp.install` (class `system`). W13 (Wave 2).
    McpInstall,
    "mcp.install",
    McpInstallParams => McpInstallResult,
    Permission::System
);

/// Params of `mcp.uninstall`: `{client, scope?: user|project, project?, dryRun?} → {changes[]}`.
pub type McpUninstallParams = Unspecified;
/// Result of `mcp.uninstall`.
pub type McpUninstallResult = Unspecified;
crate::rpc_method!(
    /// `mcp.uninstall` (class `system`). W13 (Wave 2).
    McpUninstall,
    "mcp.uninstall",
    McpUninstallParams => McpUninstallResult,
    Permission::System
);

/// Params of `mcp.http.set`: `{enabled, listen?, exposeDocker?, clientToken?} → {url, token?} (token shown once)`.
pub type McpHttpSetParams = Unspecified;
/// Result of `mcp.http.set`.
pub type McpHttpSetResult = Unspecified;
crate::rpc_method!(
    /// `mcp.http.set` (class `system`). W20 (Wave 3).
    McpHttpSet,
    "mcp.http.set",
    McpHttpSetParams => McpHttpSetResult,
    Permission::System
);

/// Params of `mcp.policy.get`: `{client?} → policy`.
pub type McpPolicyGetParams = Unspecified;
/// Result of `mcp.policy.get`.
pub type McpPolicyGetResult = Unspecified;
crate::rpc_method!(
    /// `mcp.policy.get` (class `read`). W20 (Wave 3).
    McpPolicyGet,
    "mcp.policy.get",
    McpPolicyGetParams => McpPolicyGetResult,
    Permission::Read
);

/// Params of `mcp.policy.set`: `{client, tierMax?, tools?} → policy`.
pub type McpPolicySetParams = Unspecified;
/// Result of `mcp.policy.set`.
pub type McpPolicySetResult = Unspecified;
crate::rpc_method!(
    /// `mcp.policy.set` (class `system`). W20 (Wave 3).
    McpPolicySet,
    "mcp.policy.set",
    McpPolicySetParams => McpPolicySetResult,
    Permission::System
);

/// Params of the `mcp.toolCall` notification: `{client, tool, tier, argsSummary, outcome, durationMs}; to ide`.
pub type McpToolCallParams = Unspecified;
crate::rpc_notification!(
    /// `mcp.toolCall` (daemon → client). W13.
    McpToolCall,
    "mcp.toolCall",
    McpToolCallParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] =
    &[McpStatus::INFO, McpInstall::INFO, McpUninstall::INFO, McpHttpSet::INFO, McpPolicyGet::INFO, McpPolicySet::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[McpToolCall::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
