// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! JSON-RPC transcripts: what a client sends and what it must receive, replayed against a daemon over the
//! russet/1 framing (core §5 "protocol transcripts"; CORE-15).
//!
//! A transcript is JSON Lines (`*.transcript.jsonl`); blank lines and lines starting with `//` are comments:
//!
//! ```text
//! {"transcript": 1, "name": "ping", "timeoutMs": 5000, "ignore": ["daemon.logBatch"]}
//! {"send": {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocol": {"major": 1, "minor": 0}}}}
//! {"expect": {"id": 1, "result": {"protocol": {"major": 1}, "connectionId": "$capture:conn"}}}
//! {"send": {"jsonrpc": "2.0", "id": 2, "method": "daemon.ping", "params": {}}}
//! {"expect": {"id": 2, "result": {}, "error": "$absent"}}
//! ```
//!
//! Patterns in `expect`:
//!
//! | Pattern | Matches |
//! |---|---|
//! | an object | an object with at least these keys, each matching (`"$exact": true` forbids other keys) |
//! | an array | an array of the same length, element by element; `["$contains", p…]` an array with an element matching each `p` |
//! | `"$any"` | any value that is present |
//! | `"$absent"` | (as an object value) the key is missing |
//! | `"$type:string"` | a value of that JSON type (`string`, `number`, `integer`, `boolean`, `object`, `array`, `null`) |
//! | `"$prefix:abc"` | a string starting with `abc` |
//! | `"$capture:name"` | any value, remembered as `name` |
//! | `"${name}"` | the value captured as `name` (also substituted into later `send` messages) |
//! | anything else | an equal value |
//!
//! Server messages arrive in order: each `expect` takes the next message, except that unexpected messages whose
//! `method` is listed in the header's `ignore` are skipped. A server request (`ui.confirm`) is expected like any
//! message and answered with a `send` that echoes its id through a capture.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

use crate::lsp;

/// Default time to wait for each expected message.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// One transcript line.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Send(Value),
    Expect(Value),
    Comment(String),
}

/// A parsed transcript.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    pub name: Option<String>,
    pub timeout: Option<Duration>,
    /// Methods of server messages that may arrive anywhere and are skipped when unexpected.
    pub ignore: Vec<String>,
    /// `(source line, entry)`.
    pub entries: Vec<(usize, Entry)>,
}

/// Why a replay failed; `line` is the transcript line (1-based).
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptError {
    pub line: usize,
    pub kind: ErrorKind,
}

/// What went wrong.
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorKind {
    Format(String),
    Mismatch { path: String, why: String, expected: Value, actual: Value },
    Timeout { expected: Value },
    Closed { expected: Value },
    Io(String),
}

impl fmt::Display for TranscriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Format(m) => write!(f, "transcript line {}: {m}", self.line),
            ErrorKind::Mismatch { path, why, expected, actual } => write!(
                f,
                "transcript line {}: at {path}: {why}\n  expected: {expected}\n  received: {actual}",
                self.line
            ),
            ErrorKind::Timeout { expected } => {
                write!(f, "transcript line {}: no message within the timeout; expected {expected}", self.line)
            }
            ErrorKind::Closed { expected } => {
                write!(f, "transcript line {}: the connection closed; expected {expected}", self.line)
            }
            ErrorKind::Io(e) => write!(f, "transcript line {}: I/O error: {e}", self.line),
        }
    }
}

impl std::error::Error for TranscriptError {}

/// What a successful replay saw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Replay {
    pub captures: BTreeMap<String, Value>,
    /// Every message received, in order (matched and skipped).
    pub received: Vec<Value>,
    /// The messages skipped through `ignore`.
    pub skipped: Vec<Value>,
}

impl Transcript {
    /// Parses JSON Lines.
    pub fn parse(text: &str) -> Result<Self, TranscriptError> {
        let mut t = Self::default();
        for (i, raw) in text.lines().enumerate() {
            let line = i + 1;
            let s = raw.trim();
            if s.is_empty() || s.starts_with("//") {
                continue;
            }
            let fmt_err = |m: String| TranscriptError { line, kind: ErrorKind::Format(m) };
            let v: Value = serde_json::from_str(s).map_err(|e| fmt_err(e.to_string()))?;
            let obj = v.as_object().ok_or_else(|| fmt_err("each line is a JSON object".into()))?;
            if obj.contains_key("transcript") {
                if obj["transcript"] != json!(1) {
                    return Err(fmt_err(format!("unsupported transcript version {}", obj["transcript"])));
                }
                t.name = obj.get("name").and_then(Value::as_str).map(str::to_owned);
                t.timeout = obj.get("timeoutMs").and_then(Value::as_u64).map(Duration::from_millis);
                t.ignore = obj
                    .get("ignore")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|m| m.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();
                continue;
            }
            let entry = match (obj.get("send"), obj.get("expect"), obj.get("comment")) {
                (Some(m), None, None) => Entry::Send(m.clone()),
                (None, Some(p), None) => Entry::Expect(p.clone()),
                (None, None, Some(c)) => Entry::Comment(c.as_str().unwrap_or_default().to_owned()),
                _ => return Err(fmt_err("a line has exactly one of send, expect, comment".into())),
            };
            t.entries.push((line, entry));
        }
        Ok(t)
    }

    /// Reads a transcript file.
    pub fn from_file(path: &Path) -> Result<Self, TranscriptError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| TranscriptError { line: 0, kind: ErrorKind::Io(format!("{}: {e}", path.display())) })?;
        Self::parse(&text)
    }

    /// Appends a message to send.
    pub fn send(mut self, msg: Value) -> Self {
        let line = self.entries.len() + 1;
        self.entries.push((line, Entry::Send(msg)));
        self
    }

    /// Appends a pattern to expect.
    pub fn expect(mut self, pattern: Value) -> Self {
        let line = self.entries.len() + 1;
        self.entries.push((line, Entry::Expect(pattern)));
        self
    }

    /// Methods to skip when they arrive unexpected.
    pub fn ignoring(mut self, methods: &[&str]) -> Self {
        self.ignore.extend(methods.iter().map(|m| m.to_string()));
        self
    }

    /// The transcript as JSON Lines (a header, then one entry per line).
    pub fn to_jsonl(&self) -> String {
        let mut header = Map::new();
        header.insert("transcript".into(), json!(1));
        if let Some(n) = &self.name {
            header.insert("name".into(), json!(n));
        }
        if let Some(t) = self.timeout {
            header.insert("timeoutMs".into(), json!(t.as_millis() as u64));
        }
        if !self.ignore.is_empty() {
            header.insert("ignore".into(), json!(self.ignore));
        }
        let mut out = format!("{}\n", Value::Object(header));
        for (_, e) in &self.entries {
            let v = match e {
                Entry::Send(m) => json!({ "send": m }),
                Entry::Expect(p) => json!({ "expect": p }),
                Entry::Comment(c) => json!({ "comment": c }),
            };
            out.push_str(&format!("{v}\n"));
        }
        out
    }

    /// Replays the transcript over one duplex stream (a Unix socket, an in-memory duplex).
    pub async fn replay<S: AsyncRead + AsyncWrite + Unpin>(&self, stream: S) -> Result<Replay, TranscriptError> {
        let (r, w) = tokio::io::split(stream);
        self.replay_split(r, w).await
    }

    /// Replays the transcript over a reader and a writer.
    pub async fn replay_split<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        &self,
        reader: R,
        mut writer: W,
    ) -> Result<Replay, TranscriptError> {
        let mut reader = BufReader::new(reader);
        let timeout = self.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let mut out = Replay::default();
        for (line, entry) in &self.entries {
            let line = *line;
            match entry {
                Entry::Comment(_) => {}
                Entry::Send(msg) => {
                    let msg = substitute(msg, &out.captures);
                    lsp::write_message(&mut writer, &msg)
                        .await
                        .map_err(|e| TranscriptError { line, kind: ErrorKind::Io(e.to_string()) })?;
                }
                Entry::Expect(pattern) => loop {
                    let next = tokio::time::timeout(timeout, lsp::read_message(&mut reader)).await;
                    let msg = match next {
                        Err(_) => return Err(TranscriptError { line, kind: ErrorKind::Timeout { expected: pattern.clone() } }),
                        Ok(Err(e)) => return Err(TranscriptError { line, kind: ErrorKind::Io(e.to_string()) }),
                        Ok(Ok(None)) => {
                            return Err(TranscriptError { line, kind: ErrorKind::Closed { expected: pattern.clone() } });
                        }
                        Ok(Ok(Some(m))) => m,
                    };
                    out.received.push(msg.clone());
                    let mut trial = out.captures.clone();
                    match matches(pattern, &msg, &mut trial, "$") {
                        Ok(()) => {
                            out.captures = trial;
                            break;
                        }
                        Err((path, why)) => {
                            let method = msg.get("method").and_then(Value::as_str);
                            let expected_method = pattern.get("method").and_then(Value::as_str);
                            if method.is_some_and(|m| self.ignore.iter().any(|i| i == m)) && method != expected_method {
                                out.skipped.push(msg);
                                continue;
                            }
                            return Err(TranscriptError {
                                line,
                                kind: ErrorKind::Mismatch { path, why, expected: pattern.clone(), actual: msg },
                            });
                        }
                    }
                },
            }
        }
        Ok(out)
    }

    /// Connects to a Unix socket and replays.
    #[cfg(unix)]
    pub async fn replay_unix(&self, socket: &Path) -> Result<Replay, TranscriptError> {
        let s = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|e| TranscriptError { line: 0, kind: ErrorKind::Io(format!("connecting to {}: {e}", socket.display())) })?;
        self.replay(s).await
    }
}

/// Replaces `"${name}"` strings with captured values: a whole-string reference takes the captured value as is;
/// a reference inside a longer string takes its text.
pub fn substitute(v: &Value, captures: &BTreeMap<String, Value>) -> Value {
    match v {
        Value::String(s) => {
            if let Some(name) = s.strip_prefix("${").and_then(|r| r.strip_suffix('}'))
                && let Some(c) = captures.get(name)
            {
                return c.clone();
            }
            let mut out = s.clone();
            for (k, c) in captures {
                let text = match c {
                    Value::String(t) => t.clone(),
                    other => other.to_string(),
                };
                out = out.replace(&format!("${{{k}}}"), &text);
            }
            Value::String(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, captures)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), substitute(x, captures))).collect()),
        other => other.clone(),
    }
}

/// Matches `actual` against `pattern`; on failure, the JSON path and why.
pub fn matches(
    pattern: &Value,
    actual: &Value,
    captures: &mut BTreeMap<String, Value>,
    path: &str,
) -> Result<(), (String, String)> {
    let fail = |why: String| Err((path.to_owned(), why));
    match pattern {
        Value::String(p) if p == "$any" => Ok(()),
        Value::String(p) if p == "$absent" => fail("expected no value".into()),
        Value::String(p) if p.starts_with("$capture:") => {
            captures.insert(p["$capture:".len()..].to_owned(), actual.clone());
            Ok(())
        }
        Value::String(p) if p.starts_with("${") && p.ends_with('}') => {
            let name = &p[2..p.len() - 1];
            match captures.get(name) {
                Some(c) if c == actual => Ok(()),
                Some(c) => fail(format!("expected the captured {name} = {c}")),
                None => fail(format!("nothing was captured as {name}")),
            }
        }
        Value::String(p) if p.starts_with("$type:") => {
            let ok = match &p["$type:".len()..] {
                "string" => actual.is_string(),
                "number" => actual.is_number(),
                "integer" => actual.is_i64() || actual.is_u64(),
                "boolean" => actual.is_boolean(),
                "object" => actual.is_object(),
                "array" => actual.is_array(),
                "null" => actual.is_null(),
                other => return fail(format!("unknown type {other}")),
            };
            if ok { Ok(()) } else { fail(format!("expected a value of {p}")) }
        }
        Value::String(p) if p.starts_with("$prefix:") => match actual.as_str() {
            Some(a) if a.starts_with(&p["$prefix:".len()..]) => Ok(()),
            _ => fail(format!("expected a string starting with {:?}", &p["$prefix:".len()..])),
        },
        Value::Object(po) => {
            let Some(ao) = actual.as_object() else { return fail("expected an object".into()) };
            let exact = po.get("$exact") == Some(&Value::Bool(true));
            for (k, pv) in po {
                if k == "$exact" {
                    continue;
                }
                let sub = format!("{path}.{k}");
                match (ao.get(k), pv.as_str()) {
                    (None, Some("$absent")) => {}
                    (Some(_), Some("$absent")) => return Err((sub, "expected the key to be absent".into())),
                    (None, _) => return Err((sub, "missing".into())),
                    (Some(av), _) => matches(pv, av, captures, &sub)?,
                }
            }
            if exact && let Some(extra) = ao.keys().find(|k| !po.contains_key(*k)) {
                return Err((format!("{path}.{extra}"), "unexpected key ($exact)".into()));
            }
            Ok(())
        }
        Value::Array(pa) if pa.first() == Some(&json!("$contains")) => {
            let Some(aa) = actual.as_array() else { return fail("expected an array".into()) };
            for (i, p) in pa.iter().enumerate().skip(1) {
                let found = aa.iter().any(|a| matches(p, a, &mut captures.clone(), path).is_ok());
                if !found {
                    return fail(format!("no element matches pattern {i}: {p}"));
                }
            }
            Ok(())
        }
        Value::Array(pa) => {
            let Some(aa) = actual.as_array() else { return fail("expected an array".into()) };
            if pa.len() != aa.len() {
                return fail(format!("expected {} elements, got {}", pa.len(), aa.len()));
            }
            for (i, (p, a)) in pa.iter().zip(aa).enumerate() {
                matches(p, a, captures, &format!("{path}[{i}]"))?;
            }
            Ok(())
        }
        Value::Number(want) => match actual.as_number() {
            Some(got) if got == want || got.as_f64() == want.as_f64() => Ok(()),
            _ => fail(format!("expected {want}")),
        },
        other => {
            if other == actual {
                Ok(())
            } else {
                fail(format!("expected {other}"))
            }
        }
    }
}
