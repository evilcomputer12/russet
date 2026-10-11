// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `debug` handlers (§8.5). Owner: W7 (Wave 1), W11 (Wave 2), W18 (debug.jitStats, Wave 3).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::debug as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::DebugPrepare, _, _>(not_implemented::<p::DebugPrepare>);
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
