// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The stdio bridge (ARCHITECTURE §11.1): an rmcp [`ServerHandler`] that translates each MCP tool call into
//! one daemon RPC and relays the result, with tier filtering, the 50 s rule, output caps, untrusted wrapping
//! and the approval-error mapping. Enforcement (tiers, approvals, rate limits, audit) lives in the daemon, so
//! the bridge stays a thin, faithful translator and the stdio and HTTP transports cannot diverge.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use russet_core::error::codes;
use russet_ipc::{CallOptions, Peer};
use serde_json::{Map, Value, json};

use crate::instructions::SERVER_INSTRUCTIONS;
use crate::{DEFAULT_TIER_MAX, ToolInfo, catalog, output};

/// How long the bridge waits for a daemon reply before giving up (a little over the 50 s rule, so the daemon's
/// own `{status: running}` answer wins the race; ARCHITECTURE §11.6).
const CALL_TIMEOUT: Duration = Duration::from_secs(55);

/// Tools whose textual output is device- or Apple-originated and must be wrapped (ARCHITECTURE §11.6).
const UNTRUSTED_TOOLS: &[&str] = &["logs_read", "ui_tree", "crash_read", "testflight_crashes"];

/// The MCP server handler: a translator in front of the daemon connection.
#[derive(Clone)]
pub struct RussetMcp {
    peer: Arc<Peer>,
    /// The trusted client id (`--client`); the daemon audits calls under it.
    client: String,
    /// The client's tier ceiling (ARCHITECTURE §11.3): tools above it are hidden and rejected.
    tier_max: u8,
    catalog: Arc<Vec<ToolInfo>>,
}

impl RussetMcp {
    /// A handler over an established daemon connection.
    pub fn new(peer: Arc<Peer>, client: impl Into<String>, tier_max: Option<u8>) -> Self {
        Self {
            peer,
            client: client.into(),
            tier_max: tier_max.unwrap_or(DEFAULT_TIER_MAX),
            catalog: Arc::new(catalog::tool_catalog()),
        }
    }

    /// The trusted client id this bridge presents to the daemon (`--client`).
    pub fn client(&self) -> &str {
        &self.client
    }

    /// The tools visible to this client (tier ≤ `tier_max`).
    fn visible(&self) -> Vec<&ToolInfo> {
        self.catalog.iter().filter(|t| t.tier <= self.tier_max).collect()
    }

    fn find(&self, name: &str) -> Option<&ToolInfo> {
        self.catalog.iter().find(|t| t.name == name)
    }
}

impl ServerHandler for RussetMcp {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.protocol_version = ProtocolVersion::LATEST;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("russet", env!("CARGO_PKG_VERSION"))
            .with_title("Russet")
            .with_description("Build, run, debug and ship iOS apps on a real iPhone from Linux");
        info.instructions = Some(SERVER_INSTRUCTIONS.to_owned());
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = self.visible().into_iter().map(|t| t.tool.clone()).collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.to_string();
        let Some(tool) = self.find(&name) else {
            return Err(McpError::invalid_params(Cow::Owned(format!("no MCP tool named `{name}`")), None));
        };
        if tool.tier > self.tier_max {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "tool `{name}` (tier {}) is above this client's tier-max {}; it is not available",
                tool.tier, self.tier_max
            ))])
            .into());
        }
        let method = tool.method_map[0];
        let params = request.arguments.map(Value::Object).unwrap_or_else(|| Value::Object(Map::new()));
        let opts = CallOptions { timeout: Some(CALL_TIMEOUT), ..Default::default() };
        match self.peer.call_raw(method, params, opts).await {
            Ok(value) => Ok(success_result(tool, value).into()),
            Err(rpc) => Ok(error_result(&rpc).into()),
        }
    }
}

/// Builds a successful tool result: structured content plus a short text block, wrapping untrusted output and
/// applying the 64 KiB cap (ARCHITECTURE §11.6).
fn success_result(tool: &ToolInfo, value: Value) -> CallToolResult {
    let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    let (text, truncated) = output::cap_text(&pretty);
    let text = if UNTRUSTED_TOOLS.contains(&tool.name) { output::wrap_untrusted(&text) } else { text };
    let mut blocks = vec![ContentBlock::text(text)];
    if truncated {
        blocks.push(ContentBlock::text("[output truncated at 64 KiB; narrow the query or page with a cursor]"));
    }
    let mut result = CallToolResult::success(blocks);
    result.structured_content = Some(value);
    result
}

/// Maps a daemon RPC error to a tool-level error result. `perm.approvalRequired` (1204) carries the structured
/// approval details the agent needs to retry (ARCHITECTURE §11.3).
fn error_result(rpc: &russet_ipc::RpcError) -> CallToolResult {
    let code = rpc.data.as_ref().map(|d| d.russet_code.clone()).unwrap_or_default();
    if rpc.code == codes::perm::APPROVAL_REQUIRED.number() {
        let details = rpc.data.as_ref().and_then(|d| d.details.clone()).unwrap_or(Value::Null);
        let hint = rpc
            .data
            .as_ref()
            .and_then(|d| d.hint.clone())
            .unwrap_or_else(|| "Ask the user to approve in Russet, then retry with the approval id.".to_owned());
        let mut result = CallToolResult::error(vec![ContentBlock::text(format!(
            "{}: {}. {hint}",
            codes::perm::APPROVAL_REQUIRED.as_str(),
            rpc.message
        ))]);
        result.structured_content = Some(json!({
            "error": codes::perm::APPROVAL_REQUIRED.as_str(),
            "approvalRequired": true,
            "details": details,
        }));
        return result;
    }
    let mut result = CallToolResult::error(vec![ContentBlock::text(format!("{code}: {}", rpc.message))]);
    result.structured_content = Some(json!({ "error": code, "message": rpc.message }));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use russet_core::error::Error;

    fn rpc_error(e: Error) -> russet_ipc::RpcError {
        let data = e.to_data();
        russet_ipc::RpcError { code: e.code().number(), message: e.to_string(), data: Some(Box::new(data)) }
    }

    #[test]
    fn approval_error_maps_to_structured_result() {
        let err = Error::new(codes::perm::APPROVAL_REQUIRED, "ship_upload needs approval")
            .with_details(json!({ "approvalId": "apr_X", "tier": 3 }));
        let result = error_result(&rpc_error(err));
        assert_eq!(result.is_error, Some(true));
        let sc = result.structured_content.unwrap();
        assert_eq!(sc["approvalRequired"], json!(true));
        assert_eq!(sc["details"]["approvalId"], "apr_X");
        assert_eq!(sc["error"], "perm.approvalRequired");
    }

    #[test]
    fn plain_error_keeps_the_code() {
        let err = Error::new(codes::device::LOCKED, "device is locked");
        let result = error_result(&rpc_error(err));
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["error"], "device.locked");
    }

    #[test]
    fn untrusted_tool_output_is_wrapped() {
        let tool = catalog::tool_catalog().into_iter().find(|t| t.name == "logs_read").unwrap();
        let result = success_result(&tool, json!({ "entries": ["\u{1b}[31mhi"] }));
        let text = match &result.content[0] {
            ContentBlock::Text(t) => t.text.clone(),
            _ => panic!("text block"),
        };
        assert!(text.contains("<untrusted source=\"device\">"), "{text}");
        assert!(!text.contains('\u{1b}'));
    }
}
