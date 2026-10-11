// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `forward` handlers (§8.2, §10.4). Owner: W4.
//!
//! A forward is a `forward` lease: it lives until `forward.close` or until its owner is gone (+ `lease-grace`).

use russet_core::Result;
use russet_device::rpc;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::forward as p;

use super::device::{caller, manager, owner_gone};
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ForwardOpen, _, _>(forward_open);
    r.request::<p::ForwardClose, _, _>(forward_close);
    r.request::<p::ForwardList, _, _>(forward_list);
}

async fn forward_open(cx: DaemonCtx, rq: RequestCtx, p: p::ForwardOpenParams) -> Result<p::ForwardOpenResult> {
    let m = manager(&cx).await?;
    let (result, handle) = rpc::forward_open(&m, &caller(&rq), p).await?;
    let (peer, grace) = (rq.peer.clone(), m.config().lease_grace);
    tokio::spawn(async move {
        tokio::select! {
            _ = owner_gone(&peer, grace) => {}
            _ = handle.closed() => {}
        }
        drop(handle);
    });
    Ok(result)
}

/// Only the connection that opened a forward closes it (ARCHITECTURE §6.3); `perm.denied` for anyone else.
async fn forward_close(cx: DaemonCtx, rq: RequestCtx, p: p::ForwardCloseParams) -> Result<p::ForwardCloseResult> {
    rpc::forward_close(manager(&cx).await?.as_ref(), &caller(&rq), p)
}

async fn forward_list(cx: DaemonCtx, _rq: RequestCtx, _p: p::ForwardListParams) -> Result<p::ForwardListResult> {
    Ok(rpc::forward_list(manager(&cx).await?.as_ref()))
}
