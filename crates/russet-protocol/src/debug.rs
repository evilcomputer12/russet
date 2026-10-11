// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `debug.*`: debugger-only calls (ARCHITECTURE §8.5). Owner: W7 (Wave 1), W11 (Wave 2), W18 (debug.jitStats, Wave 3).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.5 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `debug.prepare`: `run.start params + {stopOnEntry?, frontend?: dap|lldb-cli|none, jit?, symbols?, flavor?: swift|llvm, attachMode?: broker|legacy-async} → PreparedDebug`.
pub type DebugPrepareParams = Unspecified;
/// Result of `debug.prepare`.
pub type DebugPrepareResult = Unspecified;
crate::rpc_method!(
    /// `debug.prepare` (class `device`, long-running). W7 (Wave 1).
    DebugPrepare,
    "debug.prepare",
    DebugPrepareParams => DebugPrepareResult,
    Permission::Device, long_running
);

/// Params of `debug.attach`: `{udid?, pid?|bundleId?, frontend?, sessionLease?} → PreparedDebug`.
pub type DebugAttachParams = Unspecified;
/// Result of `debug.attach`.
pub type DebugAttachResult = Unspecified;
crate::rpc_method!(
    /// `debug.attach` (class `device`, long-running). W11 (Wave 2).
    DebugAttach,
    "debug.attach",
    DebugAttachParams => DebugAttachResult,
    Permission::Device, long_running
);

/// Params of `debug.control`: `{sessionId, action: continue|pause|step-over|step-into|step-out, threadId?, waitForStopMs?} → {state, stop?{reason, threadId, frames[]}}`.
pub type DebugControlParams = Unspecified;
/// Result of `debug.control`.
pub type DebugControlResult = Unspecified;
crate::rpc_method!(
    /// `debug.control` (class `device`). W11 (Wave 2).
    DebugControl,
    "debug.control",
    DebugControlParams => DebugControlResult,
    Permission::Device
);

/// Params of `debug.inspect`: `{sessionId, what: threads|stack|variables, threadId?, frameId?, scope?, depth?} → DAP-shaped data`.
pub type DebugInspectParams = Unspecified;
/// Result of `debug.inspect`.
pub type DebugInspectResult = Unspecified;
crate::rpc_method!(
    /// `debug.inspect` (class `read`). W11 (Wave 2).
    DebugInspect,
    "debug.inspect",
    DebugInspectParams => DebugInspectResult,
    Permission::Read
);

/// Params of `debug.evaluate`: `{sessionId, expression, frameId?, context?: repl|watch} → {result, type}`.
pub type DebugEvaluateParams = Unspecified;
/// Result of `debug.evaluate`.
pub type DebugEvaluateResult = Unspecified;
crate::rpc_method!(
    /// `debug.evaluate` (class `device`). W11 (Wave 2).
    DebugEvaluate,
    "debug.evaluate",
    DebugEvaluateParams => DebugEvaluateResult,
    Permission::Device
);

/// Params of `debug.breakpoint.set`: `{sessionId?, file, line, condition?, logMessage?} → {breakpointId, verified, pending}`.
pub type DebugBreakpointSetParams = Unspecified;
/// Result of `debug.breakpoint.set`.
pub type DebugBreakpointSetResult = Unspecified;
crate::rpc_method!(
    /// `debug.breakpoint.set` (class `device`). W11 (Wave 2).
    DebugBreakpointSet,
    "debug.breakpoint.set",
    DebugBreakpointSetParams => DebugBreakpointSetResult,
    Permission::Device
);

/// Params of `debug.breakpoint.remove`: `{breakpointId} → {}`.
pub type DebugBreakpointRemoveParams = Unspecified;
/// Result of `debug.breakpoint.remove`.
pub type DebugBreakpointRemoveResult = Unspecified;
crate::rpc_method!(
    /// `debug.breakpoint.remove` (class `device`). W11 (Wave 2).
    DebugBreakpointRemove,
    "debug.breakpoint.remove",
    DebugBreakpointRemoveParams => DebugBreakpointRemoveResult,
    Permission::Device
);

/// Params of `debug.jitStats`: `{sessionId} → JitStats (pagesTouched must equal pagesNotified)`.
pub type DebugJitStatsParams = Unspecified;
/// Result of `debug.jitStats`.
pub type DebugJitStatsResult = Unspecified;
crate::rpc_method!(
    /// `debug.jitStats` (class `read`). W18 (Wave 3).
    DebugJitStats,
    "debug.jitStats",
    DebugJitStatsParams => DebugJitStatsResult,
    Permission::Read
);

/// Params of `debug.symbols.status`: `{udid?} → {build, dscUuid?, tier, bytesOnDisk}`.
pub type DebugSymbolsStatusParams = Unspecified;
/// Result of `debug.symbols.status`.
pub type DebugSymbolsStatusResult = Unspecified;
crate::rpc_method!(
    /// `debug.symbols.status` (class `read`). W11 (Wave 2).
    DebugSymbolsStatus,
    "debug.symbols.status",
    DebugSymbolsStatusParams => DebugSymbolsStatusResult,
    Permission::Read
);

/// Params of `debug.symbols.plan`: `{udid?} → {files[], totalBytes, freeBytes, quotaBytes, ok, reason?}`.
pub type DebugSymbolsPlanParams = Unspecified;
/// Result of `debug.symbols.plan`.
pub type DebugSymbolsPlanResult = Unspecified;
crate::rpc_method!(
    /// `debug.symbols.plan` (class `read`). W11 (Wave 2).
    DebugSymbolsPlan,
    "debug.symbols.plan",
    DebugSymbolsPlanParams => DebugSymbolsPlanResult,
    Permission::Read
);

/// Params of `debug.symbols.fetch`: `{udid?, tier: core|full, confirmBytes?} → SymbolSet`.
pub type DebugSymbolsFetchParams = Unspecified;
/// Result of `debug.symbols.fetch`.
pub type DebugSymbolsFetchResult = Unspecified;
crate::rpc_method!(
    /// `debug.symbols.fetch` (class `device`, long-running). W11 (Wave 2).
    DebugSymbolsFetch,
    "debug.symbols.fetch",
    DebugSymbolsFetchParams => DebugSymbolsFetchResult,
    Permission::Device, long_running
);

/// Params of `debug.symbols.prune`: `{maxBytes?, build?} → {freedBytes}`.
pub type DebugSymbolsPruneParams = Unspecified;
/// Result of `debug.symbols.prune`.
pub type DebugSymbolsPruneResult = Unspecified;
crate::rpc_method!(
    /// `debug.symbols.prune` (class `system`). W11 (Wave 2).
    DebugSymbolsPrune,
    "debug.symbols.prune",
    DebugSymbolsPruneParams => DebugSymbolsPruneResult,
    Permission::System
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[
    DebugPrepare::INFO,
    DebugAttach::INFO,
    DebugControl::INFO,
    DebugInspect::INFO,
    DebugEvaluate::INFO,
    DebugBreakpointSet::INFO,
    DebugBreakpointRemove::INFO,
    DebugJitStats::INFO,
    DebugSymbolsStatus::INFO,
    DebugSymbolsPlan::INFO,
    DebugSymbolsFetch::INFO,
    DebugSymbolsPrune::INFO,
];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
