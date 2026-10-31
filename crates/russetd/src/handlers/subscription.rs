// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `subscription.*` handlers (ARCHITECTURE §8.1). Owner: W2.

use russet_core::error::Result;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::subscription::{self as p, SubscriptionCancelParams, SubscriptionCancelResult};

use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::SubscriptionCancel, _, _>(cancel);
}

async fn cancel(cx: DaemonCtx, rq: RequestCtx, p: SubscriptionCancelParams) -> Result<SubscriptionCancelResult> {
    cx.subs.cancel(&p.subscription_id, rq.peer.connection_id())?;
    Ok(SubscriptionCancelResult {})
}
