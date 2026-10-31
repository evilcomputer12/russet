// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `audit.*` handlers (ARCHITECTURE §6.5, §8.1). Owner: W2.

use russet_core::error::Result;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::audit::{self as p, AuditTailParams, AuditTailResult};

use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::AuditTail, _, _>(tail);
}

async fn tail(cx: DaemonCtx, _rq: RequestCtx, p: AuditTailParams) -> Result<AuditTailResult> {
    Ok(AuditTailResult { entries: cx.audit.tail(&p)? })
}
