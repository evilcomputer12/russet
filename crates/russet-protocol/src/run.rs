// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `run.*`: every app session: run, debug and Flutter [D 24] (ARCHITECTURE §8.5, §9.1, §9.5). Owner: W7 (Wave 1),
//! W11 (`run.relaunch`, `run.hotReload`, Wave 2), W18 (Flutter, `run.jit`, Wave 3).
//!
//! Sessions are listed with the shared `SessionSummary` of ARCHITECTURE §9.5 ([`crate::common`]); output entries
//! are the shared `LogEntry`.

use std::collections::BTreeMap;

use russet_core::ids::{SessionId, SubscriptionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{LogEntry, LogLevel, LogSource, SessionLease, SessionState, SessionSummary};
use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// When the pipeline installs (`[run] install`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum InstallMode {
    /// Only when the signed app differs from the install ledger's record (ARCHITECTURE §14.8).
    #[default]
    IfChanged,
    Always,
    Never,
}

/// How the pipeline launches (`[run] launch`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchMode {
    /// CoreDevice, falling back to DVT.
    #[default]
    Auto,
    Coredevice,
    Dvt,
    /// Build, sign and install only.
    None,
}

/// Params of `run.start`: build → provision → sign → install-if-changed → launch (ARCHITECTURE §17.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunStartParams {
    /// The absolute project root.
    pub project: String,
    /// Product, scheme or target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<String>,
    /// `debug`, `release`, `profile` or a named profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub configuration: Option<String>,
    /// UDID or alias; may be omitted when exactly one device is ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// Arguments for the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub args: Option<Vec<String>>,
    /// Environment for the app; Russet adds `NSUnbufferedIO=YES` and `OS_ACTIVITY_DT_MODE=YES` unless set here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub env: Option<BTreeMap<String, String>>,
    /// Build first (default `true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub build: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub install: Option<InstallMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub launch: Option<LaunchMode>,
    /// `owned` (default) ends with the requesting connection plus 2 s; `detached` survives it (§6.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session_lease: Option<SessionLease>,
}

/// Result of `run.start`. `sessionId` and `pid` are absent with `launch: none`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunStartResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pid: Option<u32>,
    /// The bundle ID on the device (after the bundle-ID policy).
    pub bundle_id: String,
    /// Whether this run installed the app (`false` when the ledger said it was unchanged).
    pub installed: bool,
}
crate::rpc_method!(
    /// `run.start` (class `device`, long-running). W7 (Wave 1).
    RunStart,
    "run.start",
    RunStartParams => RunStartResult,
    Permission::Device, long_running
);

/// Params of `run.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunListParams {}

/// Result of `run.list`: every session kind, detached ones included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunListResult {
    pub sessions: Vec<SessionSummary>,
}
crate::rpc_method!(
    /// `run.list` (class `read`). W7 (Wave 1).
    RunList,
    "run.list",
    RunListParams => RunListResult,
    Permission::Read
);

/// Params of `run.subscribe`: one session, or every session when omitted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunSubscribeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session_id: Option<SessionId>,
}

/// Result of `run.subscribe`; then `run.state`, `run.output`, `run.exited` (and `run.jit`, W18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunSubscribeResult {
    pub subscription_id: SubscriptionId,
}
crate::rpc_method!(
    /// `run.subscribe` (class `read`). W7 (Wave 1).
    RunSubscribe,
    "run.subscribe",
    RunSubscribeParams => RunSubscribeResult,
    Permission::Read
);

/// Which output entries `run.output.query` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct OutputFilter {
    /// Only these sources (all when omitted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sources: Option<Vec<LogSource>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub min_level: Option<LogLevel>,
    /// A plain substring of the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub subsystem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub category: Option<String>,
}

impl OutputFilter {
    /// Whether `e` passes the filter.
    pub fn matches(&self, e: &LogEntry) -> bool {
        self.sources.as_ref().is_none_or(|s| s.is_empty() || s.contains(&e.source))
            && self.min_level.is_none_or(|l| e.level >= l)
            && self.contains.as_deref().is_none_or(|c| e.message.contains(c))
            && self.subsystem.as_deref().is_none_or(|s| e.subsystem.as_deref() == Some(s))
            && self.category.as_deref().is_none_or(|c| e.category.as_deref() == Some(c))
    }
}

/// Params of `run.output.query`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunOutputQueryParams {
    pub session_id: SessionId,
    /// Only entries at or after this time: Unix epoch milliseconds (compare `LogEntry.ts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub since_ms: Option<i64>,
    /// At most this many entries, the newest ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub filter: Option<OutputFilter>,
}

/// Result of `run.output.query`: stdout and os_trace merged, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunOutputQueryResult {
    pub entries: Vec<LogEntry>,
    /// How many entries the session's 10,000-entry buffer has dropped so far.
    #[ts(type = "number")]
    pub dropped: u64,
}
crate::rpc_method!(
    /// `run.output.query` (class `read`). W7 (Wave 1).
    RunOutputQuery,
    "run.output.query",
    RunOutputQueryParams => RunOutputQueryResult,
    Permission::Read
);

/// How `run.stop` ends a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum StopMode {
    /// Kill the app.
    #[default]
    Kill,
    /// Leave the app running (a debugger detaches first); Russet stops tracking it.
    Detach,
}

/// Params of `run.stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunStopParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mode: Option<StopMode>,
}

/// Result of `run.stop`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunStopResult {}
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

/// Params of the `run.state` notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunStateParams {
    pub subscription_id: SubscriptionId,
    pub session_id: SessionId,
    pub state: SessionState,
    /// A human-readable note, for example `debugger detached`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
}
crate::rpc_notification!(
    /// `run.state` (daemon → client). W7.
    RunState,
    "run.state",
    RunStateParams
);

/// Params of the `run.output` notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunOutputParams {
    pub subscription_id: SubscriptionId,
    pub session_id: SessionId,
    pub entries: Vec<LogEntry>,
    /// Entries this subscriber lost because it was slow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub dropped: Option<u64>,
}
crate::rpc_notification!(
    /// `run.output` (daemon → client). W7.
    RunOutput,
    "run.output",
    RunOutputParams
);

/// Params of the `run.exited` notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunExitedParams {
    pub subscription_id: SubscriptionId,
    pub session_id: SessionId,
    /// The final state: `exited`, `crashed`, `lost` or `failed` (an addition to the catalog's fields).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub state: Option<SessionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub signal: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub crash_id: Option<String>,
}
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    #[test]
    fn run_start_params_use_the_catalog_names() {
        let p: RunStartParams = serde_json::from_value(json!({
            "project": "/home/u/RussetProbe", "udid": "00008101-0000000000000000", "args": ["-RussetProbeCrash", "1"],
            "env": {"FOO": "1"}, "build": false, "install": "if-changed", "launch": "coredevice",
            "sessionLease": "detached", "futureField": true}))
        .unwrap();
        assert_eq!(p.install, Some(InstallMode::IfChanged));
        assert_eq!(p.launch, Some(LaunchMode::Coredevice));
        assert_eq!(p.session_lease, Some(SessionLease::Detached));
        let minimal: RunStartParams = serde_json::from_value(json!({"project": "/p"})).unwrap();
        assert_eq!(serde_json::to_value(&minimal).unwrap(), json!({"project": "/p"}));
        assert_eq!(serde_json::to_value(LaunchMode::None).unwrap(), json!("none"));
    }

    #[test]
    fn results_and_notifications_match_the_catalog() {
        let r =
            RunStartResult { session_id: None, pid: None, bundle_id: "dev.russet.test.probe".into(), installed: false };
        assert_eq!(serde_json::to_value(&r).unwrap(), json!({"bundleId": "dev.russet.test.probe", "installed": false}));
        let q: RunOutputQueryParams = serde_json::from_value(json!({"sessionId": "ses_01J00000000000000000000000",
            "sinceMs": 1791709200123i64, "limit": 50, "filter": {"sources": ["stdout"], "minLevel": "error"}}))
        .unwrap();
        assert_eq!(q.since_ms, Some(1_791_709_200_123));
        let f = q.filter.unwrap();
        assert_eq!(f.sources, Some(vec![LogSource::Stdout]));
        let stop: RunStopParams =
            serde_json::from_value(json!({"sessionId": "ses_01J00000000000000000000000", "mode": "detach"})).unwrap();
        assert_eq!(stop.mode, Some(StopMode::Detach));
        let exited = RunExitedParams {
            subscription_id: serde_json::from_value(json!("sub_01J00000000000000000000000")).unwrap(),
            session_id: serde_json::from_value(json!("ses_01J00000000000000000000000")).unwrap(),
            state: Some(SessionState::Crashed),
            code: None,
            signal: Some(11),
            crash_id: None,
        };
        assert_eq!(
            serde_json::to_value(&exited).unwrap(),
            json!({"subscriptionId": "sub_01J00000000000000000000000", "sessionId": "ses_01J00000000000000000000000",
                   "state": "crashed", "signal": 11})
        );
    }

    #[test]
    fn output_filters() {
        let e = |source, level, msg: &str| LogEntry {
            ts: "t".into(),
            source,
            level,
            message: msg.into(),
            pid: None,
            tid: None,
            process: None,
            subsystem: Some("dev.russet.probe".into()),
            category: None,
            image_uuid: None,
            image_offset: None,
            mach_ts: None,
        };
        let all = OutputFilter::default();
        assert!(all.matches(&e(LogSource::Oslog, LogLevel::Debug, "x")));
        let f = OutputFilter {
            sources: Some(vec![LogSource::Stdout]),
            min_level: Some(LogLevel::Default),
            contains: Some("PROBE".into()),
            subsystem: Some("dev.russet.probe".into()),
            category: None,
        };
        assert!(f.matches(&e(LogSource::Stdout, LogLevel::Default, "PROBE seq=1")));
        assert!(!f.matches(&e(LogSource::Oslog, LogLevel::Default, "PROBE seq=1")));
        assert!(!f.matches(&e(LogSource::Stdout, LogLevel::Info, "PROBE seq=1")));
        assert!(!f.matches(&e(LogSource::Stdout, LogLevel::Fault, "other")));
    }

    #[test]
    fn typescript_marks_optional_fields() {
        let cfg = ts_rs::Config::default();
        let start = <RunStartParams as TS>::decl(&cfg);
        assert!(start.contains("project: string") && start.contains("sessionLease?: SessionLease"), "{start}");
        let q = <RunOutputQueryParams as TS>::decl(&cfg);
        assert!(q.contains("sinceMs?: number"), "{q}");
    }
}
