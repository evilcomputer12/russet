// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `process` handlers (§8.2). Owner: W4.

use russet_core::Result;
use russet_device::rpc;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::process as p;

use super::device::{caller, manager};
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ProcessList, _, _>(process_list);
    r.request::<p::ProcessSignal, _, _>(process_signal);
}

async fn process_list(cx: DaemonCtx, _rq: RequestCtx, p: p::ProcessListParams) -> Result<p::ProcessListResult> {
    rpc::process_list(manager(&cx).await?.as_ref(), p).await
}

async fn process_signal(cx: DaemonCtx, rq: RequestCtx, p: p::ProcessSignalParams) -> Result<p::ProcessSignalResult> {
    rpc::process_signal(manager(&cx).await?.as_ref(), &caller(&rq), p).await
}
