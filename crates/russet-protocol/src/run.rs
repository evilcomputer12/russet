// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `run.*`: every app session: run, debug and Flutter [D 24]. Owner: W7 (Wave 1), W11 (Wave 2), W18 (Flutter, Wave 3).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.5 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `run.start`: `{project, target?, configuration?, udid?, args?, env?, build?: true, install?: if-changed|always|never, launch?: auto|coredevice|dvt|none, sessionLease?: owned|detached} → {sessionId?, pid?, bundleId, installed}`.
pub type RunStartParams = Unspecified;
/// Result of `run.start`.
pub type RunStartResult = Unspecified;
crate::rpc_method!(
    /// `run.start` (class `device`, long-running). W7 (Wave 1).
    RunStart,
    "run.start",
    RunStartParams => RunStartResult,
    Permission::Device, long_running
);

/// Params of `run.list`: `{} → {sessions: SessionSummary[]}`.
pub type RunListParams = Unspecified;
/// Result of `run.list`.
pub type RunListResult = Unspecified;
crate::rpc_method!(
    /// `run.list` (class `read`). W7 (Wave 1).
    RunList,
    "run.list",
    RunListParams => RunListResult,
    Permission::Read
);

/// Params of `run.subscribe`: `{sessionId?} → {subscriptionId} ⇒ run.state, run.output, run.exited, run.jit`.
pub type RunSubscribeParams = Unspecified;
/// Result of `run.subscribe`.
pub type RunSubscribeResult = Unspecified;
crate::rpc_method!(
    /// `run.subscribe` (class `read`). W7 (Wave 1).
    RunSubscribe,
    "run.subscribe",
    RunSubscribeParams => RunSubscribeResult,
    Permission::Read
);

/// Params of `run.output.query`: `{sessionId, sinceMs?, limit?, filter?} → {entries: LogEntry[], dropped}`.
pub type RunOutputQueryParams = Unspecified;
/// Result of `run.output.query`.
pub type RunOutputQueryResult = Unspecified;
crate::rpc_method!(
    /// `run.output.query` (class `read`). W7 (Wave 1).
    RunOutputQuery,
    "run.output.query",
    RunOutputQueryParams => RunOutputQueryResult,
    Permission::Read
);

/// Params of `run.stop`: `{sessionId, mode?: kill|detach} → {}`.
pub type RunStopParams = Unspecified;
/// Result of `run.stop`.
pub type RunStopResult = Unspecified;
crate::rpc_method!(
    /// `run.stop` (class `device`). W7 (Wave 1).
    RunStop,
    "run.stop",
    RunStopParams => RunStopResult,
    Permission::Device
);

/// Params of `run.relaunch`: `{sessionId} → {pid, ms} (Swift Quick Relaunch)`.
pub type RunRelaunchParams = Unspecified;
/// Result of `run.relaunch`.
pub type RunRelaunchResult = Unspecified;
crate::rpc_method!(
    /// `run.relaunch` (class `device`, long-running). W11 (Wave 2).
    RunRelaunch,
    "run.relaunch",
    RunRelaunchParams => RunRelaunchResult,
    Permission::Device, long_running
);

/// Params of `run.hotReload`: `{sessionId, kind?: reload|restart} → {supported, kind, ms, message?}`.
pub type RunHotReloadParams = Unspecified;
/// Result of `run.hotReload`.
pub type RunHotReloadResult = Unspecified;
crate::rpc_method!(
    /// `run.hotReload` (class `device`). W11 (Swift, Wave 2) / W18 (Flutter, Wave 3).
    RunHotReload,
    "run.hotReload",
    RunHotReloadParams => RunHotReloadResult,
    Permission::Device
);

/// Params of the `run.state` notification: `{subscriptionId, sessionId, state, detail?}; to subscribers`.
pub type RunStateParams = Unspecified;
crate::rpc_notification!(
    /// `run.state` (daemon → client). W7.
    RunState,
    "run.state",
    RunStateParams
);

/// Params of the `run.output` notification: `{subscriptionId, sessionId, entries: LogEntry[], dropped?}; to subscribers`.
pub type RunOutputParams = Unspecified;
crate::rpc_notification!(
    /// `run.output` (daemon → client). W7.
    RunOutput,
    "run.output",
    RunOutputParams
);

/// Params of the `run.exited` notification: `{subscriptionId, sessionId, code?, signal?, crashId?}; to subscribers`.
pub type RunExitedParams = Unspecified;
crate::rpc_notification!(
    /// `run.exited` (daemon → client). W7.
    RunExited,
    "run.exited",
    RunExitedParams
);

/// Params of the `run.jit` notification: `{subscriptionId, sessionId, …JitStats}; to subscribers`.
pub type RunJitParams = Unspecified;
crate::rpc_notification!(
    /// `run.jit` (daemon → client). W18.
    RunJit,
    "run.jit",
    RunJitParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[
    RunStart::INFO,
    RunList::INFO,
    RunSubscribe::INFO,
    RunOutputQuery::INFO,
    RunStop::INFO,
    RunRelaunch::INFO,
    RunHotReload::INFO,
];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[RunState::INFO, RunOutput::INFO, RunExited::INFO, RunJit::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
