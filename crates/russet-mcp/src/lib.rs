// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The MCP server on rmcp 3.5.x (ARCHITECTURE §5.3 `russet-mcp`, §11; IDE §6.2): the tool catalog, the stdio
//! bridge (`russet mcp serve`) and the streamable-HTTP service that russetd mounts. The bridge connects to the
//! daemon with `client.kind = "mcp"` and `client.name = <--client id>`; it reaches the domain only through the
//! daemon (layer L4: L0–L2 plus rmcp).
//!
//! F0 stub (W1); W13 fills it (Wave 2), W20 (HTTP, Wave 3).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

use std::net::SocketAddr;
use std::sync::Arc;

use rmcp::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;

/// `russet mcp serve` options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpConfig {
    /// The `--client` id (selects the client's policy in `mcp-policy.toml`).
    pub client: Option<String>,
    /// `--http ADDR`: serve streamable HTTP in the foreground instead of stdio.
    pub http: Option<SocketAddr>,
}

/// Shared state of the HTTP service inside russetd. W20 defines the fields.
#[derive(Debug, Clone)]
pub struct HttpState {
    _f0: (),
}

/// The MCP server handler (rmcp `ServerHandler`, W13).
#[derive(Debug, Clone)]
pub struct RussetMcp {
    _f0: (),
}

/// One tool of the catalog (ARCHITECTURE §11.4).
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: &'static str,
    /// 0–3 (ARCHITECTURE §11.3).
    pub tier: u8,
    /// The RPC methods the tool calls.
    pub method_map: &'static [&'static str],
    pub tool: rmcp::model::Tool,
}

/// Serves MCP over stdio until the client disconnects.
pub async fn serve_stdio(cfg: McpConfig) -> anyhow::Result<()> {
    todo!("W13 (CORE-14, MCP-01)")
}

/// The streamable-HTTP service mounted by russetd (opt-in, `[mcp.http] enabled`).
pub fn http_service(state: HttpState) -> StreamableHttpService<RussetMcp, LocalSessionManager> {
    todo!("W20 (MCP-03)")
}

/// The canonical tool catalog.
pub fn tool_catalog() -> Vec<ToolInfo> {
    todo!("W13 (MCP-01)")
}

/// MCP doctor checks (registration status per client), for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
