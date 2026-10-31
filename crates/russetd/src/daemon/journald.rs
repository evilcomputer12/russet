// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! journald output under systemd (core §12), redacted.
//!
//! `tracing-journald` formats an event's fields itself, so it would bypass the redaction of
//! [`RedactingLayer`](super::logging::RedactingLayer). [`JournaldLayer`] instead sends the record that the
//! redaction already built (message and fields masked, the same record as the daily file and
//! `daemon.logs.subscribe` get) as one datagram of the journal's native protocol to
//! `/run/systemd/journal/socket`: `MESSAGE`, `PRIORITY`, `SYSLOG_IDENTIFIER=russetd`, `TARGET`, and every field
//! as `F_<NAME>`. The daemon uses it instead of its stderr copy when systemd connected stderr to the journal
//! (`JOURNAL_STREAM` names stderr's device and inode, systemd.exec(5)); the daily files stay.
//!
//! The layer reads span fields that the daemon's file layer recorded, so it is installed next to that layer.
//! Sending never fails the daemon: a message the socket refuses as too large is cut to [`MAX_MESSAGE`] bytes,
//! and other errors drop the line.

use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

use russet_protocol::daemon::{DaemonLogRecord, TraceLevel};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use super::logging::record_of;

/// The journal's native socket.
pub const JOURNAL_SOCKET: &str = "/run/systemd/journal/socket";

/// How much of a message is kept when the whole datagram is too large for the socket.
pub const MAX_MESSAGE: usize = 48 * 1024;

/// Whether systemd connected this process's stderr to the journal (module docs).
pub fn stderr_is_journal() -> bool {
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt;
    let Some(value) = std::env::var_os("JOURNAL_STREAM") else { return false };
    let Some((dev, ino)) = value.to_str().and_then(|v| v.split_once(':')) else { return false };
    let (Ok(dev), Ok(ino)) = (dev.parse::<u64>(), ino.parse::<u64>()) else { return false };
    let Ok(fd) = std::io::stderr().as_fd().try_clone_to_owned() else { return false };
    std::fs::File::from(fd).metadata().is_ok_and(|m| m.dev() == dev && m.ino() == ino)
}

/// A layer that sends redacted records to the journal (module docs).
#[derive(Debug)]
pub struct JournaldLayer {
    socket: Option<UnixDatagram>,
    path: PathBuf,
}

impl JournaldLayer {
    /// A layer for the system journal.
    pub fn new() -> Self {
        Self::with_socket(Path::new(JOURNAL_SOCKET))
    }

    /// A layer sending to `path` (tests use their own datagram socket).
    pub fn with_socket(path: &Path) -> Self {
        Self { socket: UnixDatagram::unbound().ok(), path: path.to_path_buf() }
    }

    fn send(&self, record: &DaemonLogRecord) {
        let Some(socket) = &self.socket else { return };
        let datagram = encode(record, None);
        if let Err(e) = socket.send_to(&datagram, &self.path)
            && e.raw_os_error() == Some(EMSGSIZE)
        {
            let _ = socket.send_to(&encode(record, Some(MAX_MESSAGE)), &self.path);
        }
    }
}

impl Default for JournaldLayer {
    fn default() -> Self {
        Self::new()
    }
}

/// `EMSGSIZE` on Linux (asm-generic/errno.h).
const EMSGSIZE: i32 = 90;

/// The syslog priority of a level, as `tracing-journald` maps it.
fn priority(level: TraceLevel) -> &'static str {
    match level {
        TraceLevel::Error => "3",
        TraceLevel::Warn => "4",
        TraceLevel::Info => "5",
        TraceLevel::Debug => "6",
        TraceLevel::Trace => "7",
    }
}

/// A journal field name for a tracing field: `F_` and the name in upper case, other characters as `_`.
fn field_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    out.push_str("F_");
    for c in name.chars().take(62) {
        out.push(if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' });
    }
    out
}

fn push_field(out: &mut Vec<u8>, name: &str, value: &str) {
    out.extend_from_slice(name.as_bytes());
    if value.contains('\n') {
        out.push(b'\n');
        out.extend_from_slice(&(value.len() as u64).to_le_bytes());
        out.extend_from_slice(value.as_bytes());
        out.push(b'\n');
    } else {
        out.push(b'=');
        out.extend_from_slice(value.as_bytes());
        out.push(b'\n');
    }
}

/// One datagram of the native protocol for `record`, with the message cut to `limit` bytes when given.
pub fn encode(record: &DaemonLogRecord, limit: Option<usize>) -> Vec<u8> {
    let mut out = Vec::with_capacity(record.message.len() + 128);
    let message = match limit {
        Some(n) if record.message.len() > n => {
            let mut end = n;
            while !record.message.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}… [cut]", &record.message[..end])
        }
        _ => record.message.clone(),
    };
    push_field(&mut out, "MESSAGE", &message);
    push_field(&mut out, "PRIORITY", priority(record.level));
    push_field(&mut out, "SYSLOG_IDENTIFIER", "russetd");
    push_field(&mut out, "TARGET", &record.target);
    for (k, v) in &record.fields {
        let value = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        push_field(&mut out, &field_name(k), &value);
    }
    out
}

/// Parses a datagram of [`encode`] back into its fields (tests).
pub fn decode(mut bytes: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let Some(nl) = bytes.iter().position(|b| *b == b'\n') else { break };
        let line = &bytes[..nl];
        if let Some(eq) = line.iter().position(|b| *b == b'=') {
            out.push((
                String::from_utf8_lossy(&line[..eq]).into_owned(),
                String::from_utf8_lossy(&line[eq + 1..]).into_owned(),
            ));
            bytes = &bytes[nl + 1..];
        } else {
            let name = String::from_utf8_lossy(line).into_owned();
            let rest = &bytes[nl + 1..];
            if rest.len() < 8 {
                break;
            }
            let mut len = [0u8; 8];
            len.copy_from_slice(&rest[..8]);
            let len = usize::try_from(u64::from_le_bytes(len)).unwrap_or(usize::MAX);
            let Some(value) = rest.get(8..8 + len) else { break };
            out.push((name, String::from_utf8_lossy(value).into_owned()));
            bytes = rest.get(8 + len + 1..).unwrap_or_default();
        }
    }
    out
}

impl<S> Layer<S> for JournaldLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        self.send(&record_of(event, &ctx));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::{EnvFilter, Registry};

    use super::*;
    use crate::daemon::logging::RedactingLayer;

    const CANARY: &str = "RuSsEtJoUrNaLcAnArY3b1";

    #[test]
    fn records_reach_the_journal_redacted_and_typed() {
        let tmp = russet_core::fsx::TempDir::new("journald").expect("tempdir");
        let path = tmp.path().join("journal.sock");
        let journal = UnixDatagram::bind(&path).expect("bind");
        journal.set_read_timeout(Some(Duration::from_secs(2))).expect("timeout");
        let subscriber = Registry::default()
            .with(EnvFilter::new("trace"))
            .with(RedactingLayer::new(std::io::sink, false))
            .with(JournaldLayer::with_socket(&path));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("rpc", conn = "c-7", token = CANARY);
            let _g = span.enter();
            tracing::warn!(password = CANARY, job = "job_1", "login\nsecond line Bearer {CANARY}");
            tracing::error!("plain");
        });
        let mut buf = vec![0u8; 65_536];
        let mut datagrams = Vec::new();
        for _ in 0..2 {
            let n = journal.recv(&mut buf).expect("a datagram");
            datagrams.push(buf[..n].to_vec());
        }
        for d in &datagrams {
            assert!(!d.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()), "the canary reached the journal");
        }
        let first = decode(&datagrams[0]);
        let get = |k: &str| first.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("PRIORITY").as_deref(), Some("4"));
        assert_eq!(get("SYSLOG_IDENTIFIER").as_deref(), Some("russetd"));
        assert_eq!(get("F_CONN").as_deref(), Some("c-7"));
        assert_eq!(get("F_JOB").as_deref(), Some("job_1"));
        assert_eq!(get("F_PASSWORD").as_deref(), Some("***"));
        assert!(get("MESSAGE").is_some_and(|m| m.starts_with("login\nsecond line")), "{first:?}");
        assert_eq!(decode(&datagrams[1]).iter().find(|(n, _)| n == "PRIORITY").map(|(_, v)| v.as_str()), Some("3"));
    }

    #[test]
    fn oversized_messages_are_cut_on_a_character_boundary() {
        let record = DaemonLogRecord {
            ts: "2026-10-11T09:00:00Z".into(),
            level: TraceLevel::Info,
            target: "russetd".into(),
            message: "é".repeat(40),
            fields: std::collections::BTreeMap::new(),
        };
        let cut = decode(&encode(&record, Some(5)));
        assert_eq!(cut[0], ("MESSAGE".to_owned(), "éé… [cut]".to_owned()));
        assert!(!stderr_is_journal() || std::env::var_os("JOURNAL_STREAM").is_some());
    }
}
