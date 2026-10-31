// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `mcp.*`: MCP administration (ARCHITECTURE §8.6). Owner: W13 (Wave 2), W20 (Wave 3).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.6 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// The registration scope of an agent client (ARCHITECTURE §11.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum McpScope {
    /// The per-user config (the stable shim as the command).
    User,
    /// A file committed in a repository (`russet` from PATH).
    Project,
}

/// Params of `mcp.status`: `{}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpStatusParams {}

/// How the stdio bridge is launched (`russet mcp serve`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpStdioInfo {
    pub command: String,
    pub args: Vec<String>,
}

/// The HTTP endpoint summary (W20; present only when enabled).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpHttpInfo {
    pub url: String,
    pub enabled: bool,
    pub listeners: Vec<String>,
}

/// One agent client's registration state (ARCHITECTURE §8.6, §11.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpClientStatus {
    /// The client id (`claude-code`, `codex`, …).
    pub id: String,
    /// Whether Russet is registered with this client (user scope).
    pub registered: bool,
    /// Whether the client appears installed.
    pub installed: bool,
    /// The scope the registration was found in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scope: Option<McpScope>,
    /// The file inspected, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
}

/// Result of `mcp.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpStatusResult {
    pub stdio: McpStdioInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub http: Option<McpHttpInfo>,
    pub clients: Vec<McpClientStatus>,
}
crate::rpc_method!(
    /// `mcp.status` (class `read`). W13 (Wave 2).
    McpStatus,
    "mcp.status",
    McpStatusParams => McpStatusResult,
    Permission::Read
);

/// One planned change to a client's config file (ARCHITECTURE §11.7; `russet-agent-config::Change`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpChange {
    /// The file the change writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    /// The client's own `mcp add` command, for reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub via_cli: Option<Vec<String>>,
    /// The file's content before the change (`None` when it did not exist).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub before: Option<String>,
    /// The file's content after the change.
    pub after: String,
}

/// Params of `mcp.install`: `{client, scope?: user|project, project?, dryRun?, tierMax?}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpInstallParams {
    /// The client id to register (`claude-code`, `codex`, `gemini`, `vscode`).
    pub client: String,
    /// `user` (default) or `project`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scope: Option<McpScope>,
    /// The project directory for `project` scope (defaults to the client's workspace).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    /// Plan only; do not write (default false).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    /// The client's tier ceiling to record in policy (ARCHITECTURE §11.3); not written by W13.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tier_max: Option<u8>,
}

/// Result of `mcp.install` / `mcp.uninstall`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpInstallResult {
    /// The changes made (or that would be made with `dryRun`); empty when already in the target state.
    pub changes: Vec<McpChange>,
    /// Whether the changes were written (`false` for a dry run or when nothing changed).
    pub applied: bool,
}
crate::rpc_method!(
    /// `mcp.install` (class `system`). W13 (Wave 2).
    McpInstall,
    "mcp.install",
    McpInstallParams => McpInstallResult,
    Permission::System
);

/// Params of `mcp.uninstall`: `{client, scope?: user|project, project?, dryRun?}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpUninstallParams {
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scope: Option<McpScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}
/// Result of `mcp.uninstall`.
pub type McpUninstallResult = McpInstallResult;
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallParams {
    /// The MCP client id that made the call (the trusted `--client` identity).
    pub client: String,
    /// The tool name (ARCHITECTURE §11.4).
    pub tool: String,
    /// The tool's tier (0–3).
    pub tier: u8,
    /// A redacted one-line summary of the arguments (no secrets or full UDIDs).
    pub args_summary: String,
    /// `ok`, `error` or `approvalRequired`.
    pub outcome: String,
    /// How long the call took.
    pub duration_ms: u64,
}
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
