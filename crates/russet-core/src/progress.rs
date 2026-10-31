// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Progress and structured job events (testability contract T7; ARCHITECTURE §9.2).
//!
//! Domain crates report through a [`ProgressSink`] (human progress: LSP work-done `begin`/`report`/`end` and
//! log lines) or an [`EventSink`] (the typed [`JobEventKind`]s of a job). The daemon's sink records events
//! in the job's `events.ndjson` and forwards progress as `$/progress` to the requester; [`NullSink`] drops
//! everything. `VecSink` and the `assert_events!` helpers live in `russet-testkit`.

use std::sync::Arc;

/// The stream a log line came from (`JobEvent` kind `log`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum LogStream {
    Stdout,
    Stderr,
    Info,
    Warn,
}

/// Human-oriented progress: LSP work-done `begin`/`report`/`end` plus log lines.
pub trait ProgressSink: Send + Sync {
    fn begin(&self, title: &str, cancellable: bool);
    fn report(&self, msg: Option<&str>, pct: Option<u8>);
    fn end(&self, msg: Option<&str>);
    fn log(&self, stream: LogStream, line: &str);
}

/// The `kind` of a `JobEvent` (ARCHITECTURE §9.2): internally tagged with `kind`, kebab-case values, struct
/// variants only, camelCase fields (ARCHITECTURE §5.2). No variant has a field named `kind`: the artifact's own
/// kind is `artifactKind` [D 96]. russet-protocol re-exports this enum; areas add fields only inside their own
/// kinds.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum JobEventKind {
    /// A line of output (every job).
    Log { stream: LogStream, line: String },
    /// Progress (every job).
    Progress {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
        done: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
        total: Option<u64>,
        /// 0.0–1.0.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        fraction: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        message: Option<String>,
    },
    /// A pipeline stage (build, run).
    Stage { stage: Stage, state: StepState },
    /// One build task (build).
    Task {
        task_id: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        tool: Option<String>,
        state: StepState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
        duration_ms: Option<u64>,
    },
    /// A compiler, signer or validator diagnostic (build, sign, ship).
    Diagnostic {
        severity: Severity,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        file: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        line: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        column: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        tool: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        code: Option<String>,
    },
    /// A produced file (build, ship). `artifactKind`, never `kind` [D 96].
    Artifact { artifact_kind: ArtifactKind, path: String },
    /// A non-fatal problem with a code (`build.warn.lint`, `debug.warn.slowLink`, …; ARCHITECTURE §13.3).
    Warning {
        russet_code: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        hint: Option<String>,
    },
    /// Bytes moved (install, upload, symbols, toolchain).
    Transfer {
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        bytes_done: u64,
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        bytes_total: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        part: Option<String>,
    },
    /// The escape hatch; each use is documented in its area doc.
    Custom { name: String, data: serde_json::Value },
}

impl JobEventKind {
    /// The wire tag (`"log"`, `"progress"`, …).
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Log { .. } => "log",
            Self::Progress { .. } => "progress",
            Self::Stage { .. } => "stage",
            Self::Task { .. } => "task",
            Self::Diagnostic { .. } => "diagnostic",
            Self::Artifact { .. } => "artifact",
            Self::Warning { .. } => "warning",
            Self::Transfer { .. } => "transfer",
            Self::Custom { .. } => "custom",
        }
    }

    /// Every wire tag, in the order of ARCHITECTURE §9.2.
    pub const TAGS: &'static [&'static str] =
        &["log", "progress", "stage", "task", "diagnostic", "artifact", "warning", "transfer", "custom"];
}

/// Pipeline stages (`JobEvent` kind `stage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Resolve,
    Prepare,
    Compile,
    Resources,
    Assemble,
    Lint,
    Package,
    Sign,
    Install,
    Launch,
    Attach,
}

/// The state of a stage or task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum StepState {
    Started,
    Finished,
    Failed,
}

/// Diagnostic severities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// Artifact kinds (`JobEvent` kind `artifact`, field `artifactKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    App,
    Ipa,
    Dsym,
    Archive,
    Report,
}

/// Structured job events on top of [`ProgressSink`].
pub trait EventSink: ProgressSink {
    fn emit(&self, e: JobEventKind);
}

/// A sink that drops everything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NullSink;

impl ProgressSink for NullSink {
    fn begin(&self, _title: &str, _cancellable: bool) {}
    fn report(&self, _msg: Option<&str>, _pct: Option<u8>) {}
    fn end(&self, _msg: Option<&str>) {}
    fn log(&self, _stream: LogStream, _line: &str) {}
}

impl EventSink for NullSink {
    fn emit(&self, _e: JobEventKind) {}
}

impl<S: ProgressSink + ?Sized> ProgressSink for Arc<S> {
    fn begin(&self, title: &str, cancellable: bool) {
        (**self).begin(title, cancellable);
    }
    fn report(&self, msg: Option<&str>, pct: Option<u8>) {
        (**self).report(msg, pct);
    }
    fn end(&self, msg: Option<&str>) {
        (**self).end(msg);
    }
    fn log(&self, stream: LogStream, line: &str) {
        (**self).log(stream, line);
    }
}

impl<S: EventSink + ?Sized> EventSink for Arc<S> {
    fn emit(&self, e: JobEventKind) {
        (**self).emit(e);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn job_events_have_the_wire_shape_of_section_9_2() {
        let cases = [
            (
                JobEventKind::Log { stream: LogStream::Stderr, line: "x".into() },
                json!({"kind": "log", "stream": "stderr", "line": "x"}),
            ),
            (
                JobEventKind::Progress { done: Some(3), total: Some(9), fraction: None, message: None },
                json!({"kind": "progress", "done": 3, "total": 9}),
            ),
            (
                JobEventKind::Stage { stage: Stage::Compile, state: StepState::Started },
                json!({"kind": "stage", "stage": "compile", "state": "started"}),
            ),
            (
                JobEventKind::Task {
                    task_id: "t1".into(),
                    title: "Compile".into(),
                    tool: Some("swiftc".into()),
                    state: StepState::Finished,
                    duration_ms: Some(12),
                },
                json!({"kind": "task", "taskId": "t1", "title": "Compile", "tool": "swiftc", "state": "finished", "durationMs": 12}),
            ),
            (
                JobEventKind::Artifact { artifact_kind: ArtifactKind::Ipa, path: "/p/App.ipa".into() },
                json!({"kind": "artifact", "artifactKind": "ipa", "path": "/p/App.ipa"}),
            ),
            (
                JobEventKind::Warning { russet_code: "build.warn.lint".into(), message: "m".into(), hint: None },
                json!({"kind": "warning", "russetCode": "build.warn.lint", "message": "m"}),
            ),
            (
                JobEventKind::Transfer { bytes_done: 5, bytes_total: 10, part: None },
                json!({"kind": "transfer", "bytesDone": 5, "bytesTotal": 10}),
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(serde_json::to_value(&event).expect("serializes"), expected);
            let back: JobEventKind = serde_json::from_value(expected).expect("deserializes");
            assert_eq!(back, event);
            assert!(JobEventKind::TAGS.contains(&event.tag()));
        }
        let d: JobEventKind = serde_json::from_value(json!({"kind": "diagnostic", "severity": "error",
            "message": "boom", "file": "a.swift", "line": 3, "futureField": true}))
        .expect("unknown fields are ignored");
        assert!(matches!(d, JobEventKind::Diagnostic { severity: Severity::Error, line: Some(3), .. }));
    }
}
