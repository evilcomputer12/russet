// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `debug` handlers (ARCHITECTURE §8.5). Owner: W7 (`debug.prepare`, Wave 1), W11 (Wave 2), W18
//! (`debug.jitStats`, Wave 3).
//!
//! `debug.prepare` runs the pipeline up to a suspended launch with the broker attached (ARCHITECTURE §17.1)
//! and returns the LLDB plan; the CLI (or the IDE) then connects.

use russet_core::Result;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::debug as p;

use super::not_implemented;
use super::run::bind_owner;
use crate::DaemonCtx;
use crate::pipeline::live::pipeline_for;
use crate::pipeline::run::TracingSink;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::DebugPrepare, _, _>(prepare);
    r.request::<p::DebugAttach, _, _>(not_implemented::<p::DebugAttach>);
    r.request::<p::DebugControl, _, _>(not_implemented::<p::DebugControl>);
    r.request::<p::DebugInspect, _, _>(not_implemented::<p::DebugInspect>);
    r.request::<p::DebugEvaluate, _, _>(not_implemented::<p::DebugEvaluate>);
    r.request::<p::DebugBreakpointSet, _, _>(not_implemented::<p::DebugBreakpointSet>);
    r.request::<p::DebugBreakpointRemove, _, _>(not_implemented::<p::DebugBreakpointRemove>);
    r.request::<p::DebugJitStats, _, _>(not_implemented::<p::DebugJitStats>);
    r.request::<p::DebugSymbolsStatus, _, _>(not_implemented::<p::DebugSymbolsStatus>);
    r.request::<p::DebugSymbolsPlan, _, _>(not_implemented::<p::DebugSymbolsPlan>);
    r.request::<p::DebugSymbolsFetch, _, _>(not_implemented::<p::DebugSymbolsFetch>);
    r.request::<p::DebugSymbolsPrune, _, _>(not_implemented::<p::DebugSymbolsPrune>);
}

async fn prepare(cx: DaemonCtx, rq: RequestCtx, params: p::DebugPrepareParams) -> Result<p::DebugPrepareResult> {
    let pipeline = pipeline_for(&cx);
    let (result, session) = pipeline.prepare_debug(params, &TracingSink, &rq.cancel).await?;
    bind_owner(&cx, &rq, &session);
    Ok(result)
}
