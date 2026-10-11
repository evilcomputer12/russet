// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Renderers (core §5.9, §12): the human reporter (anstream and indicatif) and the JSON reporter (`--json`).
//! Only this module prints to stdout.
//!
//! - Human: results as text on stdout, progress as a spinner on stderr (TTY only, not with `-q`), errors as
//!   `error: device.locked (3002): …. Hint: …` on stderr.
//! - JSON: one JSON document per result on stdout (NDJSON for streams, one item per line); errors as
//!   `{"type":"error","error":{"code","russetCode","message","hint","details"}}` on stdout (ARCHITECTURE §13.1);
//!   no progress.

// The renderers are the CLI's output.
#![allow(clippy::print_stdout, clippy::print_stderr)]

pub mod job;
pub mod prompts;

use std::io::Write;
use std::sync::Mutex;

use russet_core::Error;
use russet_core::progress::LogStream;
use russet_ipc::ProgressValue;
use russet_protocol::common::ProgressKind;

use crate::global::{ColorChoice, GlobalArgs};

/// Where a command's output goes.
pub trait Reporter: Send + Sync {
    /// `$/progress` of a call.
    fn progress(&self, p: &ProgressValue);
    /// The command's result document (`--json`), or a generic rendering of it.
    fn result(&self, v: &serde_json::Value);
    /// Text for humans; ignored with `--json`.
    fn human(&self, text: &str);
    /// One item of a stream (NDJSON with `--json`).
    fn item(&self, v: &serde_json::Value, human: &str);
    /// A log line of a job or a stream.
    fn log(&self, stream: LogStream, line: &str);
    /// A failed command.
    fn error(&self, e: &Error);
    /// Whether this is the `--json` reporter.
    fn is_json(&self) -> bool;
}

/// The reporter for these flags.
pub fn reporter_for(g: &GlobalArgs) -> std::sync::Arc<dyn Reporter> {
    if g.json {
        std::sync::Arc::new(JsonReporter::default())
    } else {
        let color = match g.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()) && russet_core::platform::stderr_is_tty()
            }
        };
        std::sync::Arc::new(HumanReporter::new(color, g.quiet))
    }
}

/// One line on stderr (prompts and notices).
pub fn eprint_line(text: &str) {
    let mut err = anstream::stderr();
    let _ = writeln!(err, "{text}");
}

/// The error line of ARCHITECTURE §13.1, for failures before a reporter exists.
pub fn print_error_line(e: &Error, json: bool) {
    if json {
        JsonReporter::default().error(e);
    } else {
        HumanReporter::new(false, false).error(e);
    }
}

/// Human output.
pub struct HumanReporter {
    color: bool,
    quiet: bool,
    spinner: Mutex<Option<indicatif::ProgressBar>>,
}

impl std::fmt::Debug for HumanReporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HumanReporter").field("color", &self.color).field("quiet", &self.quiet).finish()
    }
}

impl Default for HumanReporter {
    fn default() -> Self {
        Self::new(false, false)
    }
}

impl HumanReporter {
    pub fn new(color: bool, quiet: bool) -> Self {
        Self { color, quiet, spinner: Mutex::new(None) }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color { format!("\u{1b}[{code}m{text}\u{1b}[0m") } else { text.to_owned() }
    }

    fn finish_spinner(&self) {
        if let Some(bar) = self.spinner.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            bar.finish_and_clear();
        }
    }
}

/// A generic human rendering of a JSON value: scalars as is, objects as `key: value` lines.
pub fn render_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(m) if m.is_empty() => "ok".to_owned(),
        serde_json::Value::Object(m) => m
            .iter()
            .map(|(k, v)| match v {
                serde_json::Value::String(s) => format!("{k}: {s}"),
                other => format!("{k}: {other}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

impl Reporter for HumanReporter {
    fn progress(&self, p: &ProgressValue) {
        if self.quiet || !russet_core::platform::stderr_is_tty() {
            return;
        }
        let mut spinner = self.spinner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match p.kind {
            ProgressKind::Begin => {
                let bar = indicatif::ProgressBar::new_spinner();
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                bar.set_message(p.title.clone().unwrap_or_default());
                *spinner = Some(bar);
            }
            ProgressKind::Report => {
                if let Some(bar) = spinner.as_ref() {
                    let msg = match (p.message.as_deref(), p.percentage) {
                        (Some(m), Some(pct)) => format!("{m} ({pct}%)"),
                        (Some(m), None) => m.to_owned(),
                        (None, Some(pct)) => format!("{pct}%"),
                        (None, None) => return,
                    };
                    bar.set_message(msg);
                }
            }
            ProgressKind::End => {
                if let Some(bar) = spinner.take() {
                    bar.finish_and_clear();
                }
            }
        }
    }

    fn result(&self, v: &serde_json::Value) {
        self.human(&render_value(v));
    }

    fn human(&self, text: &str) {
        self.finish_spinner();
        if self.quiet || text.is_empty() {
            return;
        }
        let mut out = anstream::stdout();
        let _ = writeln!(out, "{text}");
    }

    fn item(&self, _v: &serde_json::Value, human: &str) {
        self.human(human);
    }

    fn log(&self, stream: LogStream, line: &str) {
        self.finish_spinner();
        if self.quiet {
            return;
        }
        match stream {
            LogStream::Stdout | LogStream::Info => {
                let mut out = anstream::stdout();
                let _ = writeln!(out, "{line}");
            }
            LogStream::Stderr | LogStream::Warn => {
                let mut err = anstream::stderr();
                let _ = writeln!(err, "{line}");
            }
        }
    }

    fn error(&self, e: &Error) {
        self.finish_spinner();
        let mut err = anstream::stderr();
        let _ = writeln!(err, "{} {}", self.paint("1;31", "error:"), e.display_line());
    }

    fn is_json(&self) -> bool {
        false
    }
}

/// `--json`: one JSON document, or NDJSON for streams.
#[derive(Debug, Default)]
pub struct JsonReporter {
    _private: (),
}

impl JsonReporter {
    fn line(v: &serde_json::Value) {
        let mut out = std::io::stdout().lock();
        let _ = serde_json::to_writer(&mut out, v);
        let _ = writeln!(out);
        let _ = out.flush();
    }
}

impl Reporter for JsonReporter {
    fn progress(&self, _p: &ProgressValue) {}

    fn result(&self, v: &serde_json::Value) {
        Self::line(v);
    }

    fn human(&self, _text: &str) {}

    fn item(&self, v: &serde_json::Value, _human: &str) {
        Self::line(v);
    }

    fn log(&self, stream: LogStream, line: &str) {
        Self::line(&serde_json::json!({"type": "log", "stream": stream, "line": line}));
    }

    fn error(&self, e: &Error) {
        let mut error = serde_json::json!({
            "code": e.code().number(),
            "russetCode": e.code().as_str(),
            "message": e.message(),
        });
        if let Some(m) = error.as_object_mut() {
            if let Some(h) = e.effective_hint() {
                m.insert("hint".into(), serde_json::Value::String(h.to_owned()));
            }
            if let Some(d) = e.details() {
                m.insert("details".into(), d.clone());
            }
        }
        Self::line(&serde_json::json!({"type": "error", "error": error}));
    }

    fn is_json(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_render_for_humans() {
        assert_eq!(render_value(&serde_json::json!({})), "ok");
        assert_eq!(render_value(&serde_json::json!({"a": "x", "b": 2})), "a: x\nb: 2");
        assert_eq!(render_value(&serde_json::json!("plain")), "plain");
    }
}
