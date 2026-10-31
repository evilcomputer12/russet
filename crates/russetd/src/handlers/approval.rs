// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `approval.*` handlers (ARCHITECTURE §6.5, §8.1, §11.3). Owner: W2.
//!
//! `approval.grant` and `approval.deny` are `system`-class calls from `ide` connections or `cli` connections with
//! a TTY only: the permission engine refuses everything else, and MCP never reaches them (ARCHITECTURE §11.5).

use std::time::Duration;

use russet_core::error::{Error, Result, codes};
use russet_core::rpc::ClientKind;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::approval::{
    self as p, Approval, ApprovalDenyParams, ApprovalGetParams, ApprovalGrantParams, ApprovalListParams,
    ApprovalListResult,
};

use crate::DaemonCtx;

/// The longest `approval.get {waitMs}` wait.
const MAX_WAIT: Duration = Duration::from_secs(10 * 60);

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ApprovalList, _, _>(list);
    r.request::<p::ApprovalGet, _, _>(get);
    r.request::<p::ApprovalGrant, _, _>(grant);
    r.request::<p::ApprovalDeny, _, _>(deny);
}

async fn list(cx: DaemonCtx, _rq: RequestCtx, p: ApprovalListParams) -> Result<ApprovalListResult> {
    Ok(ApprovalListResult { approvals: cx.approvals.list(p.state) })
}

async fn get(cx: DaemonCtx, rq: RequestCtx, p: ApprovalGetParams) -> Result<Approval> {
    match p.wait_ms {
        Some(ms) if ms > 0 => {
            let wait = Duration::from_millis(ms).min(MAX_WAIT);
            tokio::select! {
                r = cx.approvals.wait(&p.approval_id, wait) => r,
                () = rq.cancel.cancelled() => Err(Error::cancelled()),
            }
        }
        _ => cx.approvals.get(&p.approval_id),
    }
}

/// A human's connection: `ide`, or `cli` with a TTY (the engine checks it too; this is defence in depth).
fn human(rq: &RequestCtx) -> Result<()> {
    let conn = rq.peer.connection();
    if conn.kind == ClientKind::Ide || (conn.kind == ClientKind::Cli && conn.interactive) {
        Ok(())
    } else {
        Err(Error::new(codes::perm::DENIED, "approvals are granted by a human, from Russet IDE or a terminal")
            .with_hint("Run `russet approve <id>` in a terminal, or approve it in Russet IDE."))
    }
}

async fn grant(cx: DaemonCtx, rq: RequestCtx, p: ApprovalGrantParams) -> Result<Approval> {
    human(&rq)?;
    cx.approvals.grant(&p.approval_id, p.scope, rq.peer.connection())
}

async fn deny(cx: DaemonCtx, rq: RequestCtx, p: ApprovalDenyParams) -> Result<Approval> {
    human(&rq)?;
    cx.approvals.deny(&p.approval_id, p.reason, rq.peer.connection())
}
