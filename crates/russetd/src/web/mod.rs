// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The web endpoint (ARCHITECTURE §6.6): one axum listener on `127.0.0.1:0`, started lazily, with a route
//! registry. Every route checks `Origin` and `Host` and needs a token.
//!
//! F0 stub (W1); W2 owns `mod.rs` and `server.rs`; `mirror.rs` belongs to W10 and `mcp.rs` to W20.

use std::sync::Arc;

/// The opt-in streamable-HTTP MCP route (pre-created for W20).
pub mod mcp;
/// The `russet.mirror.v1` WebSocket route (pre-created for W10).
pub mod mirror;
/// The listener and route registry (W2).
pub mod server;

/// A cheap-to-clone handle on the web endpoint (`DaemonCtx.web`).
#[derive(Debug, Clone, Default)]
pub struct WebEndpoint {
    _f0: Arc<()>,
}
