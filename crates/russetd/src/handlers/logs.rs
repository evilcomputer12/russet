// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `logs` handlers (§8.2). Owner: W4.
//!
//! `logs.subscribe` streams 50 ms batches as `logs.entries` (with `dropped` after a slow consumer, ARCHITECTURE
//! §6.4) until the connection closes; `logs.tail` answers from the device's tail ring.

use russet_core::Result;
use russet_device::rpc;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::logs as p;

use super::device::manager;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::LogsSubscribe, _, _>(logs_subscribe);
    r.request::<p::LogsTail, _, _>(logs_tail);
}

async fn logs_subscribe(cx: DaemonCtx, rq: RequestCtx, p: p::LogsSubscribeParams) -> Result<p::LogsSubscribeResult> {
    let m = manager(&cx).await?;
    let mut stream = rpc::logs_subscribe(&m, &p).await?;
    let subscription_id = russet_device::ids::new_subscription_id();
    let (peer, sub) = (rq.peer.clone(), subscription_id.clone());
    tokio::spawn(async move {
        let closed = peer.closed();
        tokio::pin!(closed);
        loop {
            let batch = tokio::select! {
                _ = &mut closed => return,
                b = stream.next_batch() => b,
            };
            let Some(batch) = batch else { return };
            let params = p::LogsEntriesParams {
                subscription_id: sub.clone(),
                entries: batch.entries,
                dropped: (batch.dropped > 0).then_some(batch.dropped),
            };
            if peer.notify::<p::LogsEntries>(params).is_err() {
                return;
            }
        }
    });
    Ok(p::LogsSubscribeResult { subscription_id })
}

async fn logs_tail(cx: DaemonCtx, _rq: RequestCtx, p: p::LogsTailParams) -> Result<p::LogsTailResult> {
    rpc::logs_tail(manager(&cx).await?.as_ref(), p).await
}
