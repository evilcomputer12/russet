// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Progress and structured job events (testability contract T7; ARCHITECTURE §9.2).

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

/// The `kind` of a `JobEvent` (ARCHITECTURE §9.2): internally tagged, kebab-case, struct variants only
/// (ARCHITECTURE §5.2). russet-protocol re-exports this enum.
///
/// F0 stub: the variants carry no fields yet. W2 adds the §9.2 fields with freeze F1 (log: `stream, line`;
/// progress: `done?, total?, fraction?, message?`; stage: `stage, state`; task: `taskId, title, tool?, state,
/// durationMs?`; diagnostic: `severity, message, file?, line?, column?, tool?, code?`; artifact: `kind, path`;
/// warning: `russetCode, message, hint?`; transfer: `bytesDone, bytesTotal, part?`; custom: `name, data`).
/// Note for W2 and the lead: the artifact's own `kind` field collides with the `kind` tag and needs a
/// different wire name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum JobEventKind {
    Log {},
    Progress {},
    Stage {},
    Task {},
    Diagnostic {},
    Artifact {},
    Warning {},
    Transfer {},
    Custom {},
}

/// Structured job events on top of [`ProgressSink`].
pub trait EventSink: ProgressSink {
    fn emit(&self, e: JobEventKind);
}
