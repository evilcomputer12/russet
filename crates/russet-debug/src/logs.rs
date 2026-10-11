// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The run-output view (RD-09 minimal; RD §12): the app's stdout/stderr (CoreDevice stdio) merged with its
//! os_trace entries into one bounded, queryable stream of [`LogEntry`] (ARCHITECTURE §9.5), behind `run.output`
//! and `run.output.query`.
//!
//! - The buffer keeps the last 10,000 entries (core's subscription queue size); older ones are dropped and
//!   counted.
//! - An os_log line that `OS_ACTIVITY_DT_MODE` also mirrored to stderr arrives twice: when the same message
//!   arrives from stdio and os_trace within 50 ms, the os_trace copy is dropped (stdout wins). The dedup only
//!   sees an os_trace entry that comes second, which is the usual order over USB.
//! - Every entry is also appended to `state/sessions/<sid>/output.log` when the view has a file.

use std::collections::VecDeque;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use russet_protocol::common::{LogEntry, LogLevel, LogSource};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::broadcast;

/// The buffer size (ARCHITECTURE §6.4: 10,000 log entries).
pub const CAPACITY: usize = 10_000;
/// The stdio/os_trace dedup window.
pub const DEDUP_WINDOW: Duration = Duration::from_millis(50);

/// What `run.output.query` filters on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputFilter {
    /// Only these sources (all when empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<LogSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_level: Option<LogLevel>,
    /// A plain substring of the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subsystem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

impl OutputFilter {
    pub fn matches(&self, e: &LogEntry) -> bool {
        (self.sources.is_empty() || self.sources.contains(&e.source))
            && self.min_level.is_none_or(|l| e.level >= l)
            && self.contains.as_deref().is_none_or(|c| e.message.contains(c))
            && self.subsystem.as_deref().is_none_or(|s| e.subsystem.as_deref() == Some(s))
            && self.category.as_deref().is_none_or(|c| e.category.as_deref() == Some(c))
    }
}

struct Stamped {
    epoch_ms: i64,
    at: Instant,
    entry: LogEntry,
}

struct Ring {
    entries: VecDeque<Stamped>,
    capacity: usize,
    dropped: u64,
    next_seq: u64,
    file: Option<std::fs::File>,
}

/// The merged output of one session. Cheap to clone.
#[derive(Clone)]
pub struct LogView {
    ring: Arc<Mutex<Ring>>,
    tx: broadcast::Sender<LogEntry>,
}

impl std::fmt::Debug for LogView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let r = lock(&self.ring);
        f.debug_struct("LogView").field("len", &r.entries.len()).field("dropped", &r.dropped).finish()
    }
}

fn lock(m: &Mutex<Ring>) -> MutexGuard<'_, Ring> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Default for LogView {
    fn default() -> Self {
        Self::new(CAPACITY)
    }
}

impl LogView {
    pub fn new(capacity: usize) -> Self {
        let ring = Ring { entries: VecDeque::new(), capacity: capacity.max(1), dropped: 0, next_seq: 0, file: None };
        let (tx, _) = broadcast::channel(1024);
        Self { ring: Arc::new(Mutex::new(ring)), tx }
    }

    /// Also appends every entry to `path` (`output.log`).
    pub fn persist_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        lock(&self.ring).file = Some(f);
        Ok(())
    }

    /// Adds an entry; `false` when it was a duplicate (an os_trace copy of a stdio line).
    pub fn push(&self, entry: LogEntry) -> bool {
        let now = Instant::now();
        let mut ring = lock(&self.ring);
        if entry.source == LogSource::Oslog {
            let dup = ring.entries.iter().rev().take_while(|s| now.duration_since(s.at) <= DEDUP_WINDOW).any(|s| {
                matches!(s.entry.source, LogSource::Stdout | LogSource::Stderr) && s.entry.message == entry.message
            });
            if dup {
                return false;
            }
        }
        ring.next_seq += 1;
        if let Some(f) = ring.file.as_mut() {
            let _ = writeln!(f, "{} {:?} {:?} {}", entry.ts, entry.source, entry.level, entry.message);
        }
        if ring.entries.len() == ring.capacity {
            ring.entries.pop_front();
            ring.dropped += 1;
        }
        let epoch_ms = parse_ts_ms(&entry.ts).unwrap_or_else(now_epoch_ms);
        ring.entries.push_back(Stamped { epoch_ms, at: now, entry: entry.clone() });
        drop(ring);
        let _ = self.tx.send(entry);
        true
    }

    /// Entries at or after `since_ms` (Unix epoch milliseconds) that match `filter`, oldest first, at most
    /// `limit` (the newest ones when the limit cuts); plus how many entries the buffer has dropped so far.
    pub fn query(&self, since_ms: Option<i64>, limit: Option<usize>, filter: &OutputFilter) -> (Vec<LogEntry>, u64) {
        let ring = lock(&self.ring);
        let mut out: Vec<LogEntry> = ring
            .entries
            .iter()
            .filter(|s| since_ms.is_none_or(|t| s.epoch_ms >= t))
            .filter(|s| filter.matches(&s.entry))
            .map(|s| s.entry.clone())
            .collect();
        if let Some(limit) = limit
            && out.len() > limit
        {
            out.drain(..out.len() - limit);
        }
        (out, ring.dropped)
    }

    /// New entries as they come.
    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.tx.subscribe()
    }

    pub fn len(&self) -> usize {
        lock(&self.ring).entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn dropped(&self) -> u64 {
        lock(&self.ring).dropped
    }

    /// The sequence number the next entry gets.
    pub fn next_seq(&self) -> u64 {
        lock(&self.ring).next_seq
    }
}

/// A stdout line as a [`LogEntry`] (level `default`, ARCHITECTURE §9.5).
pub fn stdio_entry(source: LogSource, message: String, pid: Option<u32>, process: Option<&str>) -> LogEntry {
    LogEntry {
        ts: now_rfc3339(),
        source,
        level: LogLevel::Default,
        message,
        pid,
        tid: None,
        process: process.map(str::to_owned),
        subsystem: None,
        category: None,
        image_uuid: None,
        image_offset: None,
        mach_ts: None,
    }
}

/// A line from Russet itself (`source: russet`), for example "attached" or "detached".
pub fn russet_entry(level: LogLevel, message: impl Into<String>) -> LogEntry {
    LogEntry { level, source: LogSource::Russet, ..stdio_entry(LogSource::Russet, message.into(), None, None) }
}

/// Reads `reader` line by line (`\n`, with `\r` dropped, as CoreDevice's PTY sends `\r\n`) into `view` until
/// EOF. A last line without a newline is kept.
pub async fn pump_stdio<R: AsyncRead + Unpin>(view: LogView, reader: R, pid: Option<u32>, process: Option<String>) {
    let mut lines = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match lines.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                while matches!(buf.last(), Some(b'\n' | b'\r')) {
                    buf.pop();
                }
                let text = String::from_utf8_lossy(&buf).into_owned();
                view.push(stdio_entry(LogSource::Stdout, text, pid, process.as_deref()));
            }
        }
    }
}

/// Now as RFC 3339 UTC with milliseconds: `2026-10-11T09:00:00.123Z`.
pub fn now_rfc3339() -> String {
    rfc3339_ms(time::OffsetDateTime::now_utc())
}

/// RFC 3339 UTC with milliseconds.
pub fn rfc3339_ms(t: time::OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

/// Milliseconds since the Unix epoch.
pub fn now_epoch_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

/// Parses an RFC 3339 timestamp into Unix epoch milliseconds.
pub fn parse_ts_ms(ts: &str) -> Option<i64> {
    let t = time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339).ok()?;
    Some((t.unix_timestamp_nanos() / 1_000_000) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: LogSource, level: LogLevel, msg: &str) -> LogEntry {
        LogEntry { level, ..stdio_entry(source, msg.to_owned(), Some(932), Some("RussetProbe")) }
    }

    #[test]
    fn the_buffer_is_bounded_and_counts_drops() {
        let v = LogView::new(3);
        for i in 0..5 {
            v.push(entry(LogSource::Stdout, LogLevel::Default, &format!("PROBE seq={i}")));
        }
        let (entries, dropped) = v.query(None, None, &OutputFilter::default());
        assert_eq!(dropped, 2);
        let msgs: Vec<&str> = entries.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, ["PROBE seq=2", "PROBE seq=3", "PROBE seq=4"]);
        let (last, _) = v.query(None, Some(1), &OutputFilter::default());
        assert_eq!(last[0].message, "PROBE seq=4");
    }

    #[test]
    fn an_oslog_copy_of_a_stdout_line_is_dropped() {
        let v = LogView::default();
        assert!(v.push(entry(LogSource::Stdout, LogLevel::Default, "hello")));
        assert!(!v.push(entry(LogSource::Oslog, LogLevel::Default, "hello")));
        assert!(v.push(entry(LogSource::Oslog, LogLevel::Error, "other")));
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn filters_and_since() {
        let v = LogView::default();
        v.push(entry(LogSource::Stdout, LogLevel::Default, "PROBE seq=1"));
        let mut e = entry(LogSource::Oslog, LogLevel::Error, "boom");
        e.subsystem = Some("dev.russet.probe".into());
        v.push(e);
        let only_errors = OutputFilter { min_level: Some(LogLevel::Error), ..OutputFilter::default() };
        assert_eq!(v.query(None, None, &only_errors).0.len(), 1);
        let only_stdout = OutputFilter { sources: vec![LogSource::Stdout], ..OutputFilter::default() };
        assert_eq!(v.query(None, None, &only_stdout).0[0].message, "PROBE seq=1");
        let sub = OutputFilter { subsystem: Some("dev.russet.probe".into()), ..OutputFilter::default() };
        assert_eq!(v.query(None, None, &sub).0[0].message, "boom");
        let text = OutputFilter { contains: Some("seq=".into()), ..OutputFilter::default() };
        assert_eq!(v.query(None, None, &text).0.len(), 1);
        assert_eq!(v.query(Some(now_epoch_ms() + 60_000), None, &OutputFilter::default()).0.len(), 0);
        assert_eq!(v.query(Some(0), None, &OutputFilter::default()).0.len(), 2);
    }

    #[test]
    fn timestamps_are_rfc3339_with_milliseconds() {
        let ts = now_rfc3339();
        assert_eq!(ts.len(), "2026-10-11T09:00:00.123Z".len(), "{ts}");
        assert!(ts.ends_with('Z'));
        let ms = parse_ts_ms(&ts).unwrap_or(0);
        assert!((now_epoch_ms() - ms).abs() < 5_000);
        assert_eq!(parse_ts_ms("2026-10-11T09:00:00.123Z"), Some(1_791_709_200_123));
    }

    #[tokio::test]
    async fn stdio_lines_become_entries() {
        let v = LogView::default();
        let data: &[u8] = b"PROBE seq=0 t=1\r\nPROBE seq=1 t=2\r\npartial";
        pump_stdio(v.clone(), data, Some(932), Some("RussetProbe".into())).await;
        let (entries, _) = v.query(None, None, &OutputFilter::default());
        let msgs: Vec<&str> = entries.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, ["PROBE seq=0 t=1", "PROBE seq=1 t=2", "partial"]);
        assert_eq!(entries[0].pid, Some(932));
        assert_eq!(entries[0].source, LogSource::Stdout);
    }
}
