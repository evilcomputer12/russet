// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `lease.*` handlers (ARCHITECTURE §6.3, §8.1). Owner: W2.

use russet_core::error::Result;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::lease::{self as p, LeaseListParams, LeaseListResult, LeaseReleaseParams, LeaseReleaseResult};

use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::LeaseList, _, _>(list);
    r.request::<p::LeaseRelease, _, _>(release);
}

async fn list(cx: DaemonCtx, _rq: RequestCtx, _p: LeaseListParams) -> Result<LeaseListResult> {
    Ok(LeaseListResult { leases: cx.leases.list() })
}

/// The owner releases its own lease; anyone else got here with the `system` class (the permission engine), so
/// the release is forced.
async fn release(cx: DaemonCtx, rq: RequestCtx, p: LeaseReleaseParams) -> Result<LeaseReleaseResult> {
    let conn = rq.peer.connection().clone();
    let force = !cx.leases.owns(&conn.id, &p.lease_id);
    cx.leases.release(&p.lease_id, &conn, force)?;
    Ok(LeaseReleaseResult {})
}
