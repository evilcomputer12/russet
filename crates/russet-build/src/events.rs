// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Build events (ARCHITECTURE §9.2 `JobEvent` kinds) and where they go.
//!
//! [`BuildEvent`] is the build plane's view of the `JobEvent` kinds it produces (`stage`, `task`, `diagnostic`,
//! `log`, `progress`, `artifact`, `warning`), serialized with the §9.2 wire names. The [`Emitter`]:
//! - appends every event to `<project>/.russet/logs/<jobId>.jsonl` as `{seq, ts, kind, …}` (ARCHITECTURE §6.2,
//!   §14.8; build B02);
//! - forwards it to the caller's [`EventSink`]: the structured `JobEventKind` through [`EventSink::emit`] (one
//!   conversion, [`to_job_event`], so W2's F1 fields land in one place), plus human progress through the
//!   [`ProgressSink`] half (`log` for output lines, `report` for stages and progress).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::progress::{EventSink, JobEventKind, LogStream, ProgressSink};
use serde::Serialize;

/// Build stages (§9.2 `stage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Resolve,
    Prepare,
    Compile,
    Resources,
    Assemble,
    Lint,
    Package,
}

impl Stage {
    /// Every stage of a build, in order.
    pub const ALL: [Stage; 7] = [
        Stage::Resolve,
        Stage::Prepare,
        Stage::Compile,
        Stage::Resources,
        Stage::Assemble,
        Stage::Lint,
        Stage::Package,
    ];

    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Resolve => "resolve",
            Stage::Prepare => "prepare",
            Stage::Compile => "compile",
            Stage::Resources => "resources",
            Stage::Assemble => "assemble",
            Stage::Lint => "lint",
            Stage::Package => "package",
        }
    }
}

/// `stage` states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StageState {
    Started,
    Finished,
    Failed,
}

/// `task` states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    Started,
    Finished,
    Failed,
}

/// `diagnostic` severities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Note,
    Warning,
    Error,
}

/// `artifact` kinds (`artifactKind` on the wire [D 96]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    App,
    Ipa,
    Dsym,
    Archive,
    Report,
}

/// One compiler or linker diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// A build event, with the §9.2 field names.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum BuildEvent {
    Stage {
        stage: Stage,
        state: StageState,
    },
    Task {
        task_id: String,
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
        state: TaskState,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    Diagnostic {
        #[serde(flatten)]
        diagnostic: Diagnostic,
    },
    Log {
        stream: LogStream,
        line: String,
    },
    Progress {
        #[serde(skip_serializing_if = "Option::is_none")]
        done: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Artifact {
        artifact_kind: ArtifactKind,
        path: Utf8PathBuf,
    },
    Warning {
        russet_code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
}

/// The `warning` codes (`russetCode`) the build plane emits. ARCHITECTURE §13.3 registers `build.warn.lint`; the
/// others are a requested contract change (W5's report), listed here so they stay in one place.
pub mod warn {
    /// A lint finding that is not an error (build §4.7).
    pub const LINT: &str = "build.warn.lint";
    /// The backend changed (`.dynamic` products build with `native`).
    pub const BACKEND: &str = "build.warn.backend";
    /// DT* stamping under `xcode-compat`, or the diagnosis-only `RUSSET_LINK_SDK_VERSION = deployment`.
    pub const METADATA: &str = "build.warn.metadata";
    /// Resources that were not compiled or not built.
    pub const RESOURCES: &str = "build.warn.resources";
    /// An Info.plist value of the project replaced by an enforced one.
    pub const INFO_PLIST: &str = "build.warn.infoPlist";
    /// No dSYM (no `dsymutil` in the SDK bundle).
    pub const DSYM: &str = "build.warn.dsym";
    /// The executable is not stripped (no LLVM ≥ 23 `llvm-strip`).
    pub const STRIP: &str = "build.warn.strip";

    /// Every code above.
    pub const ALL: &[&str] = &[LINT, BACKEND, METADATA, RESOURCES, INFO_PLIST, DSYM, STRIP];
}

/// The structured `JobEventKind` of `e`, converted through the §9.2 wire form: [`BuildEvent`] serializes with the
/// wire names and `JobEventKind` deserializes them, ignoring fields it does not have. Against W1's F0 enum, whose
/// variants carry no fields, only the kind survives (the details reach the caller through the `ProgressSink` half
/// and the JSONL log); against W2's F1 enum, which has the §9.2 fields, every field arrives. One conversion serves
/// both, so the F1 merge needs no change here. `None` only if a kind were missing from `JobEventKind`.
pub fn to_job_event(e: &BuildEvent) -> Option<JobEventKind> {
    serde_json::to_value(e).ok().and_then(|v| serde_json::from_value(v).ok())
}

/// RFC 3339 UTC with milliseconds.
pub fn rfc3339(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let ms = d.subsec_millis();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z", rem / 3_600, (rem / 60) % 60, rem % 60)
}

/// Sends build events to the caller's sink and to the project's JSONL log.
pub struct Emitter<'a> {
    sink: &'a dyn EventSink,
    log: Mutex<Option<BufWriter<File>>>,
    log_path: Option<Utf8PathBuf>,
    seq: AtomicU64,
    diagnostics: Mutex<Vec<Diagnostic>>,
}

impl<'a> Emitter<'a> {
    /// An emitter that forwards to `sink` and, when `log_path` can be created, appends JSON lines to it.
    pub fn new(sink: &'a dyn EventSink, log_path: Option<&Utf8Path>) -> Self {
        let file = log_path.and_then(|p| {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).ok()?;
            }
            File::options().create(true).append(true).open(p).ok()
        });
        Self {
            sink,
            log_path: file.as_ref().and(log_path.map(Utf8Path::to_owned)),
            log: Mutex::new(file.map(BufWriter::new)),
            seq: AtomicU64::new(0),
            diagnostics: Mutex::new(Vec::new()),
        }
    }

    /// The JSONL log, when it could be opened.
    pub fn log_path(&self) -> Option<&Utf8Path> {
        self.log_path.as_deref()
    }

    /// Emits one event.
    pub fn emit(&self, e: BuildEvent) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut guard) = self.log.lock()
            && let Some(w) = guard.as_mut()
            && let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(&e)
        {
            let mut line = serde_json::Map::new();
            line.insert("seq".into(), seq.into());
            line.insert("ts".into(), rfc3339(SystemTime::now()).into());
            line.extend(fields);
            if serde_json::to_writer(&mut *w, &line).is_ok() {
                let _ = w.write_all(b"\n");
            }
        }
        match &e {
            BuildEvent::Log { stream, line } => self.sink.log(*stream, line),
            BuildEvent::Stage { stage, state } => {
                self.sink.report(Some(&format!("{} {}", stage.as_str(), state_word(*state))), None);
            }
            BuildEvent::Progress { done: Some(done), total: Some(total), message } if *total > 0 => {
                let pct = u8::try_from((done.saturating_mul(100) / total).min(100)).unwrap_or(100);
                self.sink.report(message.as_deref(), Some(pct));
            }
            BuildEvent::Diagnostic { diagnostic } => {
                if let Ok(mut d) = self.diagnostics.lock() {
                    d.push(diagnostic.clone());
                }
                let stream = if diagnostic.severity == Severity::Error { LogStream::Stderr } else { LogStream::Warn };
                self.sink.log(stream, &render_diagnostic(diagnostic));
            }
            BuildEvent::Warning { russet_code, message, .. } => {
                self.sink.log(LogStream::Warn, &format!("warning: {message} ({russet_code})"));
            }
            _ => {}
        }
        if let Some(kind) = to_job_event(&e) {
            self.sink.emit(kind);
        }
    }

    /// Shorthand for a `stage` event.
    pub fn stage(&self, stage: Stage, state: StageState) {
        self.emit(BuildEvent::Stage { stage, state });
    }

    /// Shorthand for a `warning` event.
    pub fn warn(&self, code: &str, message: impl Into<String>, hint: Option<&str>) {
        self.emit(BuildEvent::Warning {
            russet_code: code.to_owned(),
            message: message.into(),
            hint: hint.map(str::to_owned),
        });
    }

    /// Every diagnostic emitted so far.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.diagnostics.lock().map(|d| d.clone()).unwrap_or_default()
    }

    /// Flushes the JSONL log.
    pub fn flush(&self) {
        if let Ok(mut guard) = self.log.lock()
            && let Some(w) = guard.as_mut()
        {
            let _ = w.flush();
        }
    }
}

impl Drop for Emitter<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

fn state_word(s: StageState) -> &'static str {
    match s {
        StageState::Started => "started",
        StageState::Finished => "finished",
        StageState::Failed => "failed",
    }
}

/// `file:line:col: severity: message`, as compilers print it.
pub fn render_diagnostic(d: &Diagnostic) -> String {
    let severity = match d.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    };
    match (&d.file, d.line, d.column) {
        (Some(f), Some(l), Some(c)) => format!("{f}:{l}:{c}: {severity}: {}", d.message),
        (Some(f), Some(l), None) => format!("{f}:{l}: {severity}: {}", d.message),
        (Some(f), None, _) => format!("{f}: {severity}: {}", d.message),
        _ => format!("{severity}: {}", d.message),
    }
}

/// An [`EventSink`] that drops everything: for callers that only want the JSONL log and the result.
#[derive(Debug, Default, Clone, Copy)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_use_the_wire_names() {
        let e = BuildEvent::Artifact { artifact_kind: ArtifactKind::App, path: "/p/A.app".into() };
        assert_eq!(
            serde_json::to_value(&e).ok(),
            Some(serde_json::json!({"kind": "artifact", "artifactKind": "app", "path": "/p/A.app"}))
        );
        let d = BuildEvent::Diagnostic {
            diagnostic: Diagnostic {
                severity: Severity::Error,
                message: "boom".into(),
                file: Some("/p/a.swift".into()),
                line: Some(3),
                column: Some(7),
                tool: Some("swiftc".into()),
                code: None,
            },
        };
        assert_eq!(
            serde_json::to_value(&d).ok(),
            Some(
                serde_json::json!({"kind": "diagnostic", "severity": "error", "message": "boom", "file": "/p/a.swift", "line": 3, "column": 7, "tool": "swiftc"})
            )
        );
        let t = BuildEvent::Task {
            task_id: "t1".into(),
            title: "swift build".into(),
            tool: None,
            state: TaskState::Finished,
            duration_ms: Some(5),
        };
        assert_eq!(serde_json::to_value(&t).ok().and_then(|v| v.get("taskId").cloned()), Some("t1".into()));
        let w = BuildEvent::Warning { russet_code: "build.warn.lint".into(), message: "m".into(), hint: None };
        assert_eq!(
            serde_json::to_value(&w).ok().and_then(|v| v.get("russetCode").cloned()),
            Some("build.warn.lint".into())
        );
    }

    /// One event of every kind the build plane emits.
    fn every_kind() -> Vec<BuildEvent> {
        vec![
            BuildEvent::Stage { stage: Stage::Compile, state: StageState::Started },
            BuildEvent::Task {
                task_id: "swift-build".into(),
                title: "swift build".into(),
                tool: Some("swift".into()),
                state: TaskState::Finished,
                duration_ms: Some(12),
            },
            BuildEvent::Diagnostic {
                diagnostic: Diagnostic {
                    severity: Severity::Warning,
                    message: "unused".into(),
                    file: Some("/p/a.swift".into()),
                    line: Some(3),
                    column: Some(7),
                    tool: Some("swiftc".into()),
                    code: None,
                },
            },
            BuildEvent::Log { stream: LogStream::Stderr, line: "x".into() },
            BuildEvent::Progress { done: Some(1), total: Some(2), message: Some("Compiling".into()) },
            BuildEvent::Artifact { artifact_kind: ArtifactKind::Dsym, path: "/p/A.app.dSYM".into() },
            BuildEvent::Warning { russet_code: warn::LINT.into(), message: "m".into(), hint: Some("h".into()) },
        ]
    }

    /// Every build event becomes a `JobEventKind` of the same kind. Against the F0 enum (no fields yet) only the kind
    /// survives; against W2's F1 enum the round trip is lossless.
    #[test]
    fn job_events_keep_the_kind_and_carry_every_field_the_enum_has() {
        for e in every_kind() {
            let wire = serde_json::to_value(&e).unwrap_or_default();
            let kind = to_job_event(&e).unwrap_or_else(|| panic!("{wire} is not a JobEventKind"));
            let back = serde_json::to_value(&kind).unwrap_or_default();
            assert_eq!(back["kind"], wire["kind"]);
            let fields = back.as_object().map_or(0, serde_json::Map::len);
            assert!(fields == 1 || back == wire, "a lossy conversion: {back} from {wire}");
        }
    }

    #[test]
    fn warning_codes_are_in_the_build_family() {
        assert!(warn::ALL.iter().all(|c| c.starts_with("build.warn.")));
        let mut sorted = warn::ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), warn::ALL.len());
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(UNIX_EPOCH + std::time::Duration::from_millis(978_307_200_123)), "2001-01-01T00:00:00.123Z");
    }

    #[test]
    fn the_log_gets_seq_and_ts() {
        let dir = std::env::temp_dir().join(format!("russet-build-events-{}", std::process::id()));
        let path = Utf8PathBuf::from_path_buf(dir.join("logs/job_x.jsonl")).unwrap_or_default();
        {
            let em = Emitter::new(&NullSink, Some(&path));
            em.stage(Stage::Compile, StageState::Started);
            em.warn("build.warn.lint", "careful", None);
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let lines: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["seq"], 1);
        assert_eq!(lines[0]["kind"], "stage");
        assert_eq!(lines[0]["stage"], "compile");
        assert_eq!(lines[1]["seq"], 2);
        assert!(lines[1]["ts"].as_str().is_some_and(|t| t.ends_with('Z')));
        let _ = std::fs::remove_dir_all(dir);
    }
}
