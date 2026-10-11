// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Shared wire types (`LogEntry`, `DeviceSummary`, `JitStats`, `SessionSummary`, …; ARCHITECTURE §9.5), the
//! client references the core namespaces share, and the LSP built-ins `$/progress` and `$/cancelRequest`
//! (ARCHITECTURE §7.6). No methods. Owner: W2.
//!
//! These shapes are binding for every producer (device, run/debug) and consumer (CLI, IDE, MCP) [D 52, D 72]:
//! fields are camelCase, unit enums are kebab-case strings, optional fields are omitted rather than `null`, and
//! unknown fields are ignored. W1 landed them in F0 so that W4 (`device.list`, `logs.*`) and W7 (`run.list`,
//! `run.output`) code against the same types from day 0. Times are RFC 3339 UTC strings (ARCHITECTURE §7.3).

use russet_core::error::ErrorData;
use russet_core::ids::{JobId, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ClientKind, MethodInfo, NotificationInfo};

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];

/// A client as other clients see it: `{kind, name}` (`ui.confirm.requestedBy`, audit entries).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClientRef {
    pub kind: ClientKind,
    pub name: String,
}

/// A client connection: `{kind, name, connectionId}` (approvals' `requestedBy` and `grantedBy`, owners of jobs
/// and leases).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRef {
    pub kind: ClientKind,
    pub name: String,
    /// `c-<n>`.
    pub connection_id: String,
}

impl ConnectionRef {
    /// The `{kind, name}` part.
    pub fn client(&self) -> ClientRef {
        ClientRef { kind: self.kind, name: self.name.clone() }
    }
}

/// The token of an LSP work-done progress (`$/progress`): a string or an integer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum ProgressToken {
    Number(i64),
    String(String),
}

/// `begin`, `report` or `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ProgressKind {
    Begin,
    Report,
    End,
}

/// The value of `$/progress` (ARCHITECTURE §7.6): LSP work-done progress plus Russet's `jobId`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkDoneProgress {
    pub kind: ProgressKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
    /// 0–100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub percentage: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cancellable: Option<bool>,
    /// The job behind the request, on `begin` (ARCHITECTURE §7.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub job_id: Option<JobId>,
}

impl WorkDoneProgress {
    /// A `begin` value.
    pub fn begin(title: impl Into<String>, cancellable: bool) -> Self {
        Self {
            kind: ProgressKind::Begin,
            title: Some(title.into()),
            message: None,
            percentage: None,
            cancellable: Some(cancellable),
            job_id: None,
        }
    }

    /// A `report` value.
    pub fn report(message: Option<String>, percentage: Option<u8>) -> Self {
        Self { kind: ProgressKind::Report, title: None, message, percentage, cancellable: None, job_id: None }
    }

    /// An `end` value.
    pub fn end(message: Option<String>) -> Self {
        Self { kind: ProgressKind::End, title: None, message, percentage: None, cancellable: None, job_id: None }
    }
}

/// Params of the LSP built-in notification `$/progress` (not in the registry: its name is not `ns.event`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProgressParams {
    pub token: ProgressToken,
    pub value: WorkDoneProgress,
}

/// A JSON-RPC request id as `$/cancelRequest` carries it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum RequestIdValue {
    Number(i64),
    String(String),
}

/// Params of the LSP built-in notification `$/cancelRequest` (ARCHITECTURE §7.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CancelParams {
    pub id: RequestIdValue,
}

/// One log line: `logs.entries`, `logs.tail`, `run.output`, `run.output.query`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// RFC 3339 UTC with milliseconds, for example `2026-10-11T09:00:00.123Z`.
    pub ts: String,
    pub source: LogSource,
    pub level: LogLevel,
    /// The text (never `text`).
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub tid: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub process: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub subsystem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub image_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub image_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub mach_ts: Option<u64>,
}

/// Where a [`LogEntry`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum LogSource {
    Stdout,
    Stderr,
    Oslog,
    Syslog,
    Debugger,
    Jit,
    Russet,
}

/// Apple's log levels (never `notice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum LogLevel {
    Debug,
    Info,
    Default,
    Error,
    Fault,
}

/// One device: `device.list`, `device.changed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub udid: String,
    pub name: String,
    /// For example `iPhone13,2`.
    pub product_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub marketing_name: Option<String>,
    pub os_version: String,
    /// The iOS build, for example `24A437`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub build: Option<String>,
    pub transport: DeviceTransport,
    /// ARCHITECTURE §9.5 and §17.3: from the ChipID × iOS-major table, optionally confirmed by the probe app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code_signing_monitor: Option<CodeSigningMonitor>,
    pub state: DeviceState,
    /// Read lazily, on the first client request (ARCHITECTURE §5.3); omitted until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub readiness: Option<Readiness>,
    /// Only when `state` is `unavailable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<ErrorData>,
}

/// How a device is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceTransport {
    Usb,
    Wifi,
}

/// The device's code-signing monitor (TXM, PPL or none) [D 71].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CodeSigningMonitor {
    Txm,
    Ppl,
    None,
    Unknown,
}

/// The flat device state (russet-device's richer `DeviceState` converts to it, ARCHITECTURE §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceState {
    Attached,
    NeedsTrust,
    TrustPending,
    Ready,
    Unavailable,
}

/// What a device needs before Russet can use it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    pub paired: bool,
    pub developer_mode: Tristate,
    pub locked: Tristate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ddi: Option<DdiStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tunnel: Option<TunnelStatus>,
}

/// `yes`, `no` or `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Tristate {
    Yes,
    No,
    Unknown,
}

/// The mounted Developer Disk Image (`device.ddi.status`, `Readiness.ddi`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DdiStatus {
    pub mounted: bool,
    pub kind: DdiKind,
    /// The cryptex version, for example `27.1.266.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version: Option<String>,
    /// For example `27A266a`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub product_build: Option<String>,
    /// The source of Russet's last mount record while the mounted image still matches it, else `unknown` [D 89].
    pub source: DdiOrigin,
}

/// The DDI form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DdiKind {
    Legacy,
    PersonalizedDmg,
    Cryptex1,
}

/// Where a mounted DDI came from (Russet's `MountRecord`, ARCHITECTURE §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DdiOrigin {
    Imported,
    Dir,
    Pmd3Cache,
    Pmd3AutoMount,
    Unknown,
}

/// The userspace tunnel to the device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub state: TunnelState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub generation: Option<u64>,
    /// RFC 3339 UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub since: Option<String>,
    /// Only when `state` is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<ErrorData>,
}

/// Tunnel states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum TunnelState {
    Down,
    Up,
    Failed,
}

/// JIT servicer statistics: `debug.jitStats`, `run.jit`. On a healthy session `pagesTouched == pagesNotified`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JitStats {
    #[ts(type = "number")]
    pub stops: u64,
    #[ts(type = "number")]
    pub pages_notified: u64,
    #[ts(type = "number")]
    pub pages_touched: u64,
    #[ts(type = "number")]
    pub bytes_written: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub p50_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub p95_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rtt_ms: Option<f64>,
    pub mode: JitMode,
}

/// The JIT servicer's write mode (run-debug §7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum JitMode {
    Safe,
    Eager,
}

/// One run, debug or Flutter session: `run.list`, `russet sessions --json` [D 84].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub kind: SessionKind,
    pub bundle_id: String,
    pub udid: String,
    /// Known once the app is launched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pid: Option<u32>,
    /// The absolute project root, when the session has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    pub state: SessionState,
    pub lease: SessionLease,
    pub debugger: DebuggerState,
    /// Debug sessions only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attach_mode: Option<AttachMode>,
    /// RFC 3339 UTC.
    pub started_at: String,
    /// Only once the session has ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ended_at: Option<String>,
    /// Only once the session has ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exit: Option<SessionExit>,
}

/// Session kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SessionKind {
    Run,
    Debug,
    Flutter,
}

/// Session states (RD §17.2's Rust enum converts to it). Whether a debugger is attached is
/// [`SessionSummary::debugger`], not a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SessionState {
    Preparing,
    Building,
    Installing,
    Launching,
    Attaching,
    /// Arming the JIT hook.
    Arming,
    /// Launched suspended, waiting for a debugger.
    Ready,
    /// The app runs, with or without a debugger attached.
    Running,
    /// Stopped in the debugger.
    Stopped,
    Exited,
    Crashed,
    /// The device or the tunnel is gone.
    Lost,
    /// `run.exited` carries the details.
    Failed,
}

/// Whether a session outlives its creator (ARCHITECTURE §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SessionLease {
    Owned,
    Detached,
}

/// Whether a debugger is attached to the session's process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DebuggerState {
    None,
    Attached,
    Detached,
}

/// How LLDB reaches the app (ARCHITECTURE §10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AttachMode {
    Broker,
    LegacyAsync,
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionExit {
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Round-trips `value` through the JSON of ARCHITECTURE §9.5 (keys, enum strings, omitted optionals).
    fn same_json<T>(value: &T, expected: serde_json::Value)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        assert_eq!(serde_json::to_value(value).expect("serializes"), expected);
        let back: T = serde_json::from_value(expected).expect("deserializes");
        assert_eq!(&back, value);
    }

    #[test]
    fn log_entries_match_the_contract() {
        let entry = LogEntry {
            ts: "2026-10-11T09:00:00.123Z".into(),
            source: LogSource::Oslog,
            level: LogLevel::Default,
            message: "hello".into(),
            pid: Some(932),
            tid: None,
            process: Some("RussetProbe".into()),
            subsystem: None,
            category: None,
            image_uuid: None,
            image_offset: Some(4096),
            mach_ts: None,
        };
        same_json(
            &entry,
            json!({"ts": "2026-10-11T09:00:00.123Z", "source": "oslog", "level": "default", "message": "hello",
                   "pid": 932, "process": "RussetProbe", "imageOffset": 4096}),
        );
        // Unknown fields are ignored (ARCHITECTURE §7.3).
        let extra: LogEntry = serde_json::from_value(json!({"ts": "t", "source": "jit", "level": "fault",
            "message": "m", "futureField": 1}))
        .expect("unknown fields are ignored");
        assert_eq!(extra.source, LogSource::Jit);
    }

    #[test]
    fn device_summaries_match_the_contract() {
        let device = DeviceSummary {
            udid: "00008101-0000000000000000".into(),
            name: "iPhone".into(),
            product_type: "iPhone13,2".into(),
            marketing_name: Some("iPhone 12".into()),
            os_version: "27.0".into(),
            build: Some("24A437".into()),
            transport: DeviceTransport::Usb,
            code_signing_monitor: Some(CodeSigningMonitor::Txm),
            state: DeviceState::TrustPending,
            readiness: Some(Readiness {
                paired: true,
                developer_mode: Tristate::Yes,
                locked: Tristate::Unknown,
                ddi: Some(DdiStatus {
                    mounted: true,
                    kind: DdiKind::PersonalizedDmg,
                    version: Some("27.1.266.1".into()),
                    product_build: Some("27A266a".into()),
                    source: DdiOrigin::Pmd3AutoMount,
                }),
                tunnel: Some(TunnelStatus {
                    state: TunnelState::Up,
                    generation: Some(3),
                    since: Some("2026-10-11T09:00:00Z".into()),
                    error: None,
                }),
            }),
            error: None,
        };
        same_json(
            &device,
            json!({"udid": "00008101-0000000000000000", "name": "iPhone", "productType": "iPhone13,2",
                   "marketingName": "iPhone 12", "osVersion": "27.0", "build": "24A437", "transport": "usb",
                   "codeSigningMonitor": "txm", "state": "trust-pending",
                   "readiness": {"paired": true, "developerMode": "yes", "locked": "unknown",
                                 "ddi": {"mounted": true, "kind": "personalized-dmg", "version": "27.1.266.1",
                                         "productBuild": "27A266a", "source": "pmd3-auto-mount"},
                                 "tunnel": {"state": "up", "generation": 3, "since": "2026-10-11T09:00:00Z"}}}),
        );
    }

    #[test]
    fn jit_stats_and_sessions_match_the_contract() {
        let stats = JitStats {
            stops: 41,
            pages_notified: 1312,
            pages_touched: 1312,
            bytes_written: 5640,
            p50_ms: Some(0.5),
            p95_ms: Some(2.0),
            rtt_ms: None,
            mode: JitMode::Safe,
        };
        same_json(
            &stats,
            json!({"stops": 41, "pagesNotified": 1312, "pagesTouched": 1312, "bytesWritten": 5640,
                   "p50Ms": 0.5, "p95Ms": 2.0, "mode": "safe"}),
        );
        let session: SessionSummary = serde_json::from_value(json!({
            "sessionId": "ses_01J00000000000000000000000", "kind": "debug", "bundleId": "dev.russet.test.probe",
            "udid": "00008101-0000000000000000", "pid": 932, "project": "/home/u/RussetProbe", "state": "running",
            "lease": "detached", "debugger": "detached", "attachMode": "legacy-async", "startedAt": "t0"}))
        .expect("a SessionSummary of ARCHITECTURE §9.5");
        assert_eq!(
            (session.kind, session.state, session.lease, session.debugger, session.attach_mode),
            (
                SessionKind::Debug,
                SessionState::Running,
                SessionLease::Detached,
                DebuggerState::Detached,
                Some(AttachMode::LegacyAsync)
            )
        );
        let ended = SessionSummary {
            state: SessionState::Crashed,
            ended_at: Some("t1".into()),
            exit: Some(SessionExit { code: None, signal: Some(9), crash_id: Some("crash-1".into()) }),
            ..session
        };
        let value = serde_json::to_value(&ended).expect("serializes");
        assert_eq!(value["state"], "crashed");
        assert_eq!(value["exit"], json!({"signal": 9, "crashId": "crash-1"}));
    }

    #[test]
    fn optional_fields_are_optional_in_typescript() {
        let cfg = ts_rs::Config::default();
        let log = <LogEntry as TS>::decl(&cfg);
        for field in ["ts: string", "message: string", "tid?: number", "imageOffset?: number", "pid?: number"] {
            assert!(log.contains(field), "{field} missing in {log}");
        }
        let jit = <JitStats as TS>::decl(&cfg);
        assert!(jit.contains("stops: number") && jit.contains("p50Ms?: number"), "{jit}");
        let session = <SessionSummary as TS>::decl(&cfg);
        assert!(session.contains("endedAt?: string") && session.contains("state: SessionState"), "{session}");
    }

    #[test]
    fn every_session_state_has_its_wire_name() {
        let states = [
            (SessionState::Preparing, "preparing"),
            (SessionState::Building, "building"),
            (SessionState::Installing, "installing"),
            (SessionState::Launching, "launching"),
            (SessionState::Attaching, "attaching"),
            (SessionState::Arming, "arming"),
            (SessionState::Ready, "ready"),
            (SessionState::Running, "running"),
            (SessionState::Stopped, "stopped"),
            (SessionState::Exited, "exited"),
            (SessionState::Crashed, "crashed"),
            (SessionState::Lost, "lost"),
            (SessionState::Failed, "failed"),
        ];
        for (state, name) in states {
            assert_eq!(serde_json::to_value(state).expect("serializes"), json!(name));
        }
    }
}
