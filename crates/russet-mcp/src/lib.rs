// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The MCP server on rmcp 3.5.x (ARCHITECTURE §5.3 `russet-mcp`, §11; IDE §6.2): the tool catalog, the stdio
//! bridge (`russet mcp serve`) and the streamable-HTTP service that russetd mounts. The bridge connects to the
//! daemon with `client.kind = "mcp"` and `client.name = <--client id>`; it reaches the domain only through the
//! daemon (layer L4: L0–L2 plus rmcp).
//!
//! - [`catalog`] is the canonical tool list of ARCHITECTURE §11.4 (names, tiers, RPC maps, schemas, `_meta`).
//! - [`snapshot`] freezes it to `schemas/mcp/tools.v1.json` (the F4 contract snapshot).
//! - [`bridge`] ([`RussetMcp`]) translates MCP tool calls into daemon RPCs with tier filtering, the 50 s rule,
//!   output caps, untrusted wrapping and the approval-error mapping ([`output`], [`instructions`]).
//! - [`serve_stdio`] stands the bridge up over stdio for `russet mcp serve`.
//! - HTTP (`http_service`, [`HttpState`]) is W20 (Wave 3).

// Tests use unwrap/expect freely (the repo convention for test code).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::net::SocketAddr;
use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use russet_core::dirs::RussetDirs;
use russet_core::rpc::ClientKind;
use russet_ipc::{ClientInfo, SpawnSpec};

pub mod bridge;
pub mod catalog;
pub mod checks;
pub mod instructions;
pub mod output;
pub mod snapshot;

pub use bridge::RussetMcp;
pub use catalog::tool_catalog as catalog_tools;
pub use instructions::SERVER_INSTRUCTIONS;

/// The default tier ceiling when a client's policy sets none (ARCHITECTURE §11.3, §14.6: `default-tier-max`).
pub const DEFAULT_TIER_MAX: u8 = 1;

/// `russet mcp serve` options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpConfig {
    /// The `--client` id (selects the client's policy in `mcp-policy.toml`, and is the trusted identity).
    pub client: Option<String>,
    /// `--http ADDR`: serve streamable HTTP in the foreground instead of stdio (W20).
    pub http: Option<SocketAddr>,
    /// The client's tier ceiling; `None` uses [`DEFAULT_TIER_MAX`].
    pub tier_max: Option<u8>,
}

/// Shared state of the HTTP service inside russetd. W20 defines the fields.
#[derive(Debug, Clone)]
pub struct HttpState {
    _f0: (),
}

/// One tool of the catalog (ARCHITECTURE §11.4).
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: &'static str,
    /// 0–3 (ARCHITECTURE §11.3).
    pub tier: u8,
    /// The RPC methods the tool calls (the first is the primary method the bridge forwards to).
    pub method_map: &'static [&'static str],
    pub tool: rmcp::model::Tool,
}

/// The `daemon run --detach` spawn spec of this executable (so the bridge can autostart the daemon,
/// ARCHITECTURE §3.3). Mirrors the CLI's `spawn::spawn_spec`.
fn daemon_spawn_spec() -> anyhow::Result<SpawnSpec> {
    let executable = std::env::current_exe()?;
    let expect = if russet_core::build_info::is_development() {
        Some(russet_ipc::DevelopmentCheck::for_this_process().map_err(|e| anyhow::anyhow!(e.to_string()))?)
    } else {
        None
    };
    Ok(SpawnSpec {
        executable,
        args: vec!["daemon".into(), "run".into(), "--detach".into()],
        ready_timeout: std::time::Duration::from_secs(10),
        expect,
    })
}

/// Serves MCP over stdio until the client disconnects (`russet mcp serve`). Connects to the daemon as a
/// `mcp` client named after `--client`, so the daemon enforces tiers, approvals, rate limits and audit
/// (ARCHITECTURE §11.1). Logs go to stderr, never stdout (ARCHITECTURE §11.2).
pub async fn serve_stdio(cfg: McpConfig) -> anyhow::Result<()> {
    let client_id = cfg.client.clone().unwrap_or_else(|| "mcp".to_owned());
    let dirs = RussetDirs::from_env().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut client = ClientInfo::new(ClientKind::Mcp, client_id.clone());
    // A device reservation shared by the IDE (ARCHITECTURE §11.7: `RUSSET_LEASE`).
    client.shared_lease = std::env::var("RUSSET_LEASE")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .and_then(|v| russet_core::ids::LeaseId::parse(v.trim()).ok());

    let (peer, _init) = russet_ipc::connect_or_spawn(&dirs, client, daemon_spawn_spec()?)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let handler = RussetMcp::new(Arc::new(peer), client_id, cfg.tier_max);
    let running = handler.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}

/// The streamable-HTTP service mounted by russetd (opt-in, `[mcp.http] enabled`).
pub fn http_service(state: HttpState) -> StreamableHttpService<RussetMcp, LocalSessionManager> {
    let _ = state;
    todo!("W20 (MCP-03)")
}

/// The canonical tool catalog (ARCHITECTURE §11.4).
pub fn tool_catalog() -> Vec<ToolInfo> {
    catalog::tool_catalog()
}

/// MCP doctor checks (registration status per client), for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    checks::checks()
}
