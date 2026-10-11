// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `initialize` handlers (ARCHITECTURE §7.4). Owner: W2.
//!
//! The server pipeline has already checked the TCP token, computed the grant and recorded the session; this
//! handler checks the protocol major, registers the connection (for `daemon.status`, broadcasts and idle exit),
//! records a shared device reservation, arranges the cleanup when the connection closes, and answers with the
//! daemon's identity. On a protocol major mismatch an idle daemon (no jobs, leases or other connections)
//! answers `ipc.protocolMismatch` with `restartable: true` and shuts down, so the client can start its own
//! build (ARCHITECTURE §3.3 item 3). A daemon that is shutting down answers `ipc.daemonUnavailable` with
//! `details.shuttingDown` (the client waits for it to go and starts a new one; core §6.3).

use std::time::Duration;

use russet_core::error::{Error, Result, codes};
use russet_ipc::{RequestCtx, Router};
use russet_protocol::PROTOCOL;
use russet_protocol::daemon::ShutdownReason;
use russet_protocol::initialize::{
    self as p, DaemonUnavailableDetails, InitializeParams, InitializeResult, ProtocolMismatchDetails,
    ServerCapabilities, ServerEndpoints, ServerFeatures,
};

use crate::DaemonCtx;
use crate::daemon::SHUTDOWN_GRACE;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::Initialize, _, _>(initialize);
}

async fn initialize(cx: DaemonCtx, rq: RequestCtx, params: InitializeParams) -> Result<InitializeResult> {
    let session = rq.peer.session().cloned().ok_or_else(|| Error::internal("initialize without a session"))?;
    if cx.daemon.is_shutting_down() {
        let (_, force) = cx.daemon.shutdown_reason();
        let details = DaemonUnavailableDetails {
            shutting_down: true,
            grace_ms: Some(if force { 0 } else { russet_core::time::millis(SHUTDOWN_GRACE) }),
        };
        return Err(Error::new(codes::ipc::DAEMON_UNAVAILABLE, "the daemon is shutting down")
            .with_hint("Retry in a moment: `russet` waits for it to stop and starts a new daemon.")
            .with_details(serde_json::to_value(details).unwrap_or_default())
            .retryable(true));
    }
    if params.protocol.major != PROTOCOL.major {
        let counts = cx.jobs.counts();
        let restartable =
            counts.queued + counts.running == 0 && cx.leases.count() == 0 && cx.daemon.connections.count() == 0;
        if restartable {
            let daemon = cx.daemon.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                daemon.request_shutdown(ShutdownReason::Upgrade, false);
            });
        }
        let details =
            ProtocolMismatchDetails { server_protocol: PROTOCOL, client_protocol: params.protocol, restartable };
        return Err(Error::new(
            codes::ipc::PROTOCOL_MISMATCH,
            format!("this daemon speaks russet/{}, the client russet/{}", PROTOCOL.major, params.protocol.major),
        )
        .with_hint(if restartable {
            "The idle daemon is shutting down; the client starts its own build."
        } else {
            "The daemon is busy: let its jobs finish, then `russet daemon restart --force`."
        })
        .with_details(serde_json::to_value(details).unwrap_or_default()));
    }
    let conn_id = session.conn.id.clone();
    cx.daemon.connections.register(&rq.peer);
    if let Some(lease) = &params.shared_lease {
        cx.leases.add_sharer(lease, &conn_id);
    }
    {
        let (cx, peer, id) = (cx.clone(), rq.peer.clone(), conn_id.clone());
        tokio::spawn(async move {
            peer.closed().await;
            cx.daemon.connections.remove(&id);
            cx.subs.close_connection(&id);
            cx.leases.owner_gone(&id);
            cx.approvals.connection_closed(&id);
            tracing::debug!(conn = %id, "connection closed");
        });
    }
    tracing::debug!(conn = %conn_id, kind = ?session.conn.kind, name = %session.conn.name, "connection initialized");
    Ok(InitializeResult {
        protocol: PROTOCOL,
        server: cx.daemon.server_info(),
        connection_id: conn_id,
        grant: session.grant,
        capabilities: ServerCapabilities {
            methods: russet_protocol::registry().iter().map(|m| m.name.to_owned()).collect(),
            notifications: russet_protocol::notifications().iter().map(|n| n.name.to_owned()).collect(),
            features: ServerFeatures { mirror: false, wifi_devices: false, mcp_http: false },
        },
        endpoints: ServerEndpoints { web: cx.web.url() },
    })
}
