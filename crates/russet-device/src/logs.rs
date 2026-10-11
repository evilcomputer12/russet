// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Device logs: os_trace by pid, syslog, stdout merge (DEV-09; device.md §6.7). The wire shape is `LogEntry`
//! (ARCHITECTURE §9.5).
//!
//! - `oslog`: `os_trace_relay` over lockdown (survives tunnel resets), filtered by pid on the device.
//! - `syslog`: `syslog_relay` text lines, parsed best-effort.
//! - `stdout`: the console of a process Russet launched (the reliable source of app text on iOS 26+).
//!
//! A [`LogStream`] batches entries in 50 ms windows and keeps at most 10,000 pending entries; a slow consumer loses
//! the oldest and the next batch reports `dropped` (ARCHITECTURE §6.4). [`LogCapture`] keeps a 20,000-entry ring per
//! device for `logs.tail`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use idevice::Idevice;
use idevice::os_trace_relay::{OsTraceLog, OsTraceRelayClient};
use idevice::syslog_relay::SyslogRelayClient;
use russet_core::Result;
use russet_protocol::common::{LogEntry, LogLevel, LogSource};
use time::OffsetDateTime;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::error::from_idevice;
use crate::manager::DeviceHandle;
use crate::pairing::LABEL;

/// The batching window (device.md §6.7).
pub const BATCH_WINDOW: Duration = Duration::from_millis(50);
/// Pending entries per stream before the oldest are dropped (ARCHITECTURE §6.4).
pub const STREAM_CAPACITY: usize = 10_000;
/// The `logs.tail` ring per capture (device.md §6.7).
pub const TAIL_CAPACITY: usize = 20_000;
/// A tail capture stops after this long without a `logs.tail` call.
pub const TAIL_IDLE: Duration = Duration::from_secs(120);

/// Which sources a stream reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogSourceKind {
    OsLog,
    Syslog,
    Stdout,
}

/// Which log entries to deliver.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFilter {
    /// Device-side for `oslog`; host-side for the rest.
    pub pid: Option<u32>,
    /// Resolved to running pids when the stream opens.
    pub bundle_id: Option<String>,
    /// Process name (host-side).
    pub process: Option<String>,
    /// Subsystem prefix (host-side).
    pub subsystem: Option<String>,
    pub category: Option<String>,
    pub min_level: Option<LogLevel>,
    /// Case-insensitive substring of the message.
    pub contains: Option<String>,
    /// Empty means `oslog` and `stdout`.
    pub sources: Vec<LogSourceKind>,
}

impl LogFilter {
    /// The host-side test (the pid filter applies to `oslog` on the device already, and to everything here).
    pub fn matches(&self, e: &LogEntry) -> bool {
        if let Some(pid) = self.pid
            && e.pid != Some(pid)
        {
            return false;
        }
        if let Some(p) = &self.process
            && e.process.as_deref() != Some(p.as_str())
        {
            return false;
        }
        if let Some(s) = &self.subsystem
            && !e.subsystem.as_deref().is_some_and(|x| x.starts_with(s.as_str()))
        {
            return false;
        }
        if let Some(c) = &self.category
            && e.category.as_deref() != Some(c.as_str())
        {
            return false;
        }
        if let Some(min) = self.min_level
            && e.level < min
        {
            return false;
        }
        if let Some(needle) = &self.contains
            && !e.message.to_lowercase().contains(&needle.to_lowercase())
        {
            return false;
        }
        true
    }

    fn wants(&self, k: LogSourceKind) -> bool {
        if self.sources.is_empty() {
            matches!(k, LogSourceKind::OsLog | LogSourceKind::Stdout)
        } else {
            self.sources.contains(&k)
        }
    }
}

/// RFC 3339 UTC with milliseconds (`2026-10-11T09:00:00.123Z`).
pub fn rfc3339_millis(t: OffsetDateTime) -> String {
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

/// Apple's os_trace levels → `LogLevel` (`Notice` is `default`; never "notice" on the wire).
pub fn level_of(l: idevice::os_trace_relay::LogLevel) -> LogLevel {
    use idevice::os_trace_relay::LogLevel as L;
    match l {
        L::Notice => LogLevel::Default,
        L::Info => LogLevel::Info,
        L::Debug => LogLevel::Debug,
        L::Error => LogLevel::Error,
        L::Fault => LogLevel::Fault,
    }
}

/// Maps one os_trace entry (device.md §6.7 format) to the wire `LogEntry`.
pub fn entry_from_os_trace(l: &OsTraceLog) -> LogEntry {
    let ts = l.timestamp.and_utc();
    let ts = OffsetDateTime::from_unix_timestamp_nanos(
        i128::from(ts.timestamp()) * 1_000_000_000 + i128::from(ts.timestamp_subsec_nanos()),
    )
    .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let process = l.filename.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_owned);
    LogEntry {
        ts: rfc3339_millis(ts),
        source: LogSource::Oslog,
        level: level_of(l.level),
        message: l.message.clone(),
        pid: Some(l.pid),
        tid: (l.thread_id != 0).then_some(l.thread_id),
        process,
        subsystem: l.label.as_ref().map(|x| x.subsystem.clone()).filter(|s| !s.is_empty()),
        category: l.label.as_ref().map(|x| x.category.clone()).filter(|s| !s.is_empty()),
        image_uuid: (!l.image_uuid.is_nil()).then(|| l.image_uuid.hyphenated().to_string().to_uppercase()),
        image_offset: (l.image_offset != 0).then_some(u64::from(l.image_offset)),
        mach_ts: (l.mach_timestamp != 0).then_some(l.mach_timestamp),
    }
}

/// Parses one `syslog_relay` line (`Oct 11 09:00:00 <host> Process(Subsystem)[pid] <Level>: message`). The host
/// name (the device name, personal data) is dropped.
pub fn entry_from_syslog(line: &str, now: OffsetDateTime) -> LogEntry {
    let mut entry = LogEntry {
        ts: rfc3339_millis(now),
        source: LogSource::Syslog,
        level: LogLevel::Default,
        message: line.to_owned(),
        pid: None,
        tid: None,
        process: None,
        subsystem: None,
        category: None,
        image_uuid: None,
        image_offset: None,
        mach_ts: None,
    };
    // Skip "Mon DD HH:MM:SS host ".
    let mut parts = line.splitn(5, ' ').collect::<Vec<_>>();
    if parts.len() == 5 && parts[1].is_empty() {
        // Single-digit days are padded: "Oct  1 09:00:00 host rest".
        parts = line.splitn(6, ' ').filter(|s| !s.is_empty()).collect();
        if parts.len() < 5 {
            return entry;
        }
    }
    let Some(rest) = parts.get(4) else { return entry };
    let Some((head, message)) = rest.split_once(": ") else { return entry };
    let (head, level) = match head.rfind(" <") {
        Some(i) if head.ends_with('>') => (&head[..i], Some(&head[i + 2..head.len() - 1])),
        _ => (head, None),
    };
    let (proc_part, pid) = match head.rfind('[') {
        Some(i) if head.ends_with(']') => (&head[..i], head[i + 1..head.len() - 1].parse::<u32>().ok()),
        _ => (head, None),
    };
    let (process, subsystem) = match proc_part.find('(') {
        Some(i) if proc_part.ends_with(')') => (&proc_part[..i], Some(&proc_part[i + 1..proc_part.len() - 1])),
        _ => (proc_part, None),
    };
    entry.process = (!process.is_empty()).then(|| process.to_owned());
    entry.subsystem = subsystem.filter(|s| !s.is_empty()).map(str::to_owned);
    entry.pid = pid;
    entry.level = match level {
        Some("Debug") => LogLevel::Debug,
        Some("Info") => LogLevel::Info,
        Some("Error") => LogLevel::Error,
        Some("Fault") | Some("Critical") | Some("Alert") | Some("Emergency") => LogLevel::Fault,
        _ => LogLevel::Default,
    };
    entry.message = message.to_owned();
    entry
}

/// One delivered batch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogBatch {
    pub entries: Vec<LogEntry>,
    /// Entries lost since the previous batch (slow consumer).
    pub dropped: u64,
}

/// The pending queue of a stream: bounded, dropping the oldest.
#[derive(Debug)]
pub struct Batcher {
    queue: Mutex<VecDeque<LogEntry>>,
    dropped: AtomicU64,
    notify: Notify,
    capacity: usize,
    closed: std::sync::atomic::AtomicBool,
}

impl Batcher {
    pub fn new(capacity: usize) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            dropped: AtomicU64::new(0),
            notify: Notify::new(),
            capacity,
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Queues an entry; when full, the oldest goes and counts as dropped.
    pub fn push(&self, e: LogEntry) {
        if let Ok(mut q) = self.queue.lock() {
            if q.len() >= self.capacity {
                q.pop_front();
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            q.push_back(e);
        }
        self.notify.notify_one();
    }

    /// Ends the stream after the queued entries.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    fn take(&self) -> LogBatch {
        let entries = self.queue.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default();
        LogBatch { entries, dropped: self.dropped.swap(0, Ordering::Relaxed) }
    }

    fn is_empty(&self) -> bool {
        self.queue.lock().map(|q| q.is_empty()).unwrap_or(true)
    }

    /// The next batch: waits for an entry, then for the rest of the window. `None` once closed and drained.
    pub async fn next(&self, window: Duration) -> Option<LogBatch> {
        loop {
            if !self.is_empty() {
                tokio::time::sleep(window).await;
                return Some(self.take());
            }
            if self.closed.load(Ordering::SeqCst) {
                let b = self.take();
                return (!b.entries.is_empty() || b.dropped > 0).then_some(b);
            }
            self.notify.notified().await;
        }
    }
}

/// A live log stream; dropping it stops its readers.
#[derive(Debug)]
pub struct LogStream {
    batcher: Arc<Batcher>,
    stop: CancellationToken,
}

impl LogStream {
    /// The next 50 ms batch; `None` when every source has ended.
    pub async fn next_batch(&mut self) -> Option<LogBatch> {
        self.batcher.next(BATCH_WINDOW).await
    }

    /// Stops the readers.
    pub fn close(&self) {
        self.stop.cancel();
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// Opens the sources of `f` and merges them.
pub async fn open(h: &DeviceHandle, mut f: LogFilter) -> Result<LogStream> {
    let batcher = Arc::new(Batcher::new(STREAM_CAPACITY));
    let stop = CancellationToken::new();
    let mut pids: Vec<u32> = f.pid.into_iter().collect();
    if pids.is_empty()
        && let Some(b) = f.bundle_id.clone()
    {
        pids = crate::process::pids_of(h, &b).await.unwrap_or_default();
        if let Some(first) = pids.first() {
            f.pid = Some(*first);
        } else if let Ok(Some(app)) = crate::apps::lookup(h, &b).await {
            // Not running yet: match the executable's name until it starts.
            f.process = app.executable;
        }
    }
    let readers = Arc::new(AtomicU64::new(0));
    if f.wants(LogSourceKind::OsLog) {
        let stream = h.conn().lockdown_service(h.udid(), "com.apple.os_trace_relay").await?;
        let client = OsTraceRelayClient { idevice: Idevice::new(stream, LABEL) };
        let mut rx = client.start_trace(f.pid).await.map_err(|e| from_idevice(e, "starting os_trace_relay"))?;
        let (b, s, filt, r) = (Arc::clone(&batcher), stop.clone(), f.clone(), Arc::clone(&readers));
        r.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(async move {
            loop {
                let next = tokio::select! {
                    _ = s.cancelled() => break,
                    n = rx.next() => n,
                };
                match next {
                    Ok(l) => {
                        let e = entry_from_os_trace(&l);
                        if filt.matches(&e) {
                            b.push(e);
                        }
                    }
                    Err(_) => break,
                }
            }
            if r.fetch_sub(1, Ordering::SeqCst) == 1 {
                b.close();
            }
        });
    }
    if f.wants(LogSourceKind::Syslog) {
        let stream = h.conn().lockdown_service(h.udid(), "com.apple.syslog_relay").await?;
        let mut client = SyslogRelayClient::new(Idevice::new(stream, LABEL));
        let (b, s, filt, r) = (Arc::clone(&batcher), stop.clone(), f.clone(), Arc::clone(&readers));
        r.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(async move {
            loop {
                let next = tokio::select! {
                    _ = s.cancelled() => break,
                    n = client.next() => n,
                };
                match next {
                    Ok(line) => {
                        let e = entry_from_syslog(line.trim_end(), OffsetDateTime::now_utc());
                        if filt.matches(&e) {
                            b.push(e);
                        }
                    }
                    Err(_) => break,
                }
            }
            if r.fetch_sub(1, Ordering::SeqCst) == 1 {
                b.close();
            }
        });
    }
    if f.wants(LogSourceKind::Stdout) {
        let hubs: Vec<_> = pids.iter().filter_map(|p| h.actor.output(*p)).collect();
        for hub in hubs {
            let (backlog, mut rx) = hub.subscribe();
            let (b, s, filt, r) = (Arc::clone(&batcher), stop.clone(), f.clone(), Arc::clone(&readers));
            r.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                for e in backlog {
                    if filt.matches(&e) {
                        b.push(e);
                    }
                }
                loop {
                    let next = tokio::select! {
                        _ = s.cancelled() => break,
                        n = rx.recv() => n,
                    };
                    match next {
                        Ok(e) => {
                            if filt.matches(&e) {
                                b.push(e);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            b.dropped.fetch_add(n, Ordering::Relaxed);
                        }
                        Err(_) => break,
                    }
                }
                if r.fetch_sub(1, Ordering::SeqCst) == 1 {
                    b.close();
                }
            });
        }
    }
    if readers.load(Ordering::SeqCst) == 0 {
        batcher.close();
    }
    Ok(LogStream { batcher, stop })
}

/// A background capture feeding the `logs.tail` ring.
#[derive(Debug)]
pub struct LogCapture {
    ring: Mutex<VecDeque<(u64, LogEntry)>>,
    seq: AtomicU64,
    last_used: Mutex<Instant>,
    started: Instant,
    stop: CancellationToken,
}

impl LogCapture {
    fn push(&self, e: LogEntry) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut r) = self.ring.lock() {
            if r.len() == TAIL_CAPACITY {
                r.pop_front();
            }
            r.push_back((seq, e));
        }
    }

    fn touch(&self) {
        if let Ok(mut l) = self.last_used.lock() {
            *l = Instant::now();
        }
    }

    fn idle(&self) -> bool {
        self.last_used.lock().map(|l| l.elapsed() >= TAIL_IDLE).unwrap_or(true)
    }

    /// Stops the capture.
    pub fn stop(&self) {
        self.stop.cancel();
    }
}

/// A `logs.tail` query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TailQuery {
    pub bundle_id: Option<String>,
    pub pid: Option<u32>,
    /// Default 200, at most 2000.
    pub lines: Option<u32>,
    pub min_level: Option<LogLevel>,
    pub contains: Option<String>,
    /// RFC 3339; only entries at or after it.
    pub since: Option<String>,
    /// Only entries after this cursor (from a previous result).
    pub cursor: Option<String>,
    /// How long a new capture may collect before answering (default 1.5 s).
    pub first_wait: Option<Duration>,
}

/// A `logs.tail` answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TailResult {
    pub entries: Vec<LogEntry>,
    /// More entries matched than `lines`.
    pub truncated: bool,
    pub cursor: Option<String>,
}

/// Picks entries from a ring: filter, then the last `lines`.
pub fn select_tail(ring: &[(u64, LogEntry)], q: &TailQuery) -> TailResult {
    let lines = q.lines.unwrap_or(200).clamp(1, 2000) as usize;
    let after: u64 = q.cursor.as_deref().and_then(|c| c.parse().ok()).unwrap_or(0);
    let filter = LogFilter { min_level: q.min_level, contains: q.contains.clone(), ..Default::default() };
    let matched: Vec<&(u64, LogEntry)> = ring
        .iter()
        .filter(|(seq, e)| *seq > after && filter.matches(e) && q.since.as_deref().is_none_or(|s| e.ts.as_str() >= s))
        .collect();
    let truncated = matched.len() > lines;
    let picked = &matched[matched.len().saturating_sub(lines)..];
    let cursor = picked.last().map(|(s, _)| s.to_string()).or_else(|| q.cursor.clone());
    TailResult { entries: picked.iter().map(|(_, e)| e.clone()).collect(), truncated, cursor }
}

/// `logs.tail`: reads the ring of the matching capture, starting one (and waiting `first_wait`) when none runs.
pub async fn tail(h: &DeviceHandle, q: TailQuery) -> Result<TailResult> {
    let key = match (q.pid, q.bundle_id.as_deref()) {
        (Some(p), _) => format!("pid:{p}"),
        (None, Some(b)) => format!("bundle:{b}"),
        _ => "all".to_owned(),
    };
    let existing = h.actor.captures.lock().ok().and_then(|m| m.get(&key).cloned());
    let capture = match existing {
        Some(c) if !c.stop.is_cancelled() => c,
        _ => {
            let filter = LogFilter {
                pid: q.pid,
                bundle_id: q.bundle_id.clone(),
                sources: vec![LogSourceKind::OsLog, LogSourceKind::Stdout],
                ..Default::default()
            };
            let mut stream = open(h, filter).await?;
            let capture = Arc::new(LogCapture {
                ring: Mutex::new(VecDeque::new()),
                seq: AtomicU64::new(0),
                last_used: Mutex::new(Instant::now()),
                started: Instant::now(),
                stop: CancellationToken::new(),
            });
            let c = Arc::clone(&capture);
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = c.stop.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(10)) => {
                            if c.idle() {
                                c.stop.cancel();
                                break;
                            }
                        }
                        b = stream.next_batch() => match b {
                            Some(b) => for e in b.entries { c.push(e) },
                            None => break,
                        },
                    }
                }
                c.stop.cancel();
            });
            if let Ok(mut m) = h.actor.captures.lock() {
                m.retain(|_, c| !c.stop.is_cancelled());
                m.insert(key, Arc::clone(&capture));
            }
            capture
        }
    };
    capture.touch();
    let wait = q.first_wait.unwrap_or(Duration::from_millis(1500));
    let elapsed = capture.started.elapsed();
    if elapsed < wait {
        tokio::time::sleep(wait - elapsed).await;
    }
    let ring: Vec<(u64, LogEntry)> = capture.ring.lock().map(|r| r.iter().cloned().collect()).unwrap_or_default();
    Ok(select_tail(&ring, &q))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(level: LogLevel, msg: &str) -> LogEntry {
        LogEntry {
            ts: "2026-10-11T09:00:00.000Z".into(),
            source: LogSource::Oslog,
            level,
            message: msg.into(),
            pid: Some(932),
            tid: None,
            process: Some("RussetProbe".into()),
            subsystem: Some("dev.russet.probe".into()),
            category: Some("ui".into()),
            image_uuid: None,
            image_offset: None,
            mach_ts: None,
        }
    }

    #[test]
    fn os_trace_entries_map_to_log_entries() {
        // idevice's `OsTraceLog` (de)serializes with serde; its timestamp is a chrono `NaiveDateTime`.
        let l: OsTraceLog = serde_json::from_value(serde_json::json!({
            "pid": 932,
            "timestamp": "2026-10-11T09:00:00.123",
            "level": "Notice",
            "image_name": "/usr/lib/libSystem.B.dylib",
            "filename": "/private/var/containers/Bundle/Application/X/RussetProbe.app/RussetProbe",
            "message": "hello",
            "label": {"subsystem": "dev.russet.probe", "category": "ui"},
            "procid": 932,
            "thread_id": 7,
            "image_offset": 4096,
            "image_uuid": "abababab-abab-abab-abab-abababababab",
            "process_image_uuid": "00000000-0000-0000-0000-000000000000",
            "mach_timestamp": 123456789u64
        }))
        .expect("an os_trace entry");
        let entry = entry_from_os_trace(&l);
        let json = serde_json::to_value(&entry).expect("json");
        assert_eq!(
            json,
            serde_json::json!({"ts": "2026-10-11T09:00:00.123Z", "source": "oslog", "level": "default", "message": "hello",
                               "pid": 932, "tid": 7, "process": "RussetProbe", "subsystem": "dev.russet.probe", "category": "ui",
                               "imageUuid": "ABABABAB-ABAB-ABAB-ABAB-ABABABABABAB", "imageOffset": 4096, "machTs": 123456789})
        );
    }

    #[test]
    fn levels_use_apple_names() {
        use idevice::os_trace_relay::LogLevel as L;
        assert_eq!(level_of(L::Notice), LogLevel::Default);
        assert_eq!(level_of(L::Fault), LogLevel::Fault);
        assert_eq!(serde_json::to_value(level_of(L::Notice)).expect("json"), "default");
    }

    #[test]
    fn syslog_lines_parse_and_drop_the_host_name() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let x = entry_from_syslog("Oct 11 09:00:00 MyPhone SpringBoard(FrontBoard)[57] <Notice>: Hello there", now);
        assert_eq!(x.process.as_deref(), Some("SpringBoard"));
        assert_eq!(x.subsystem.as_deref(), Some("FrontBoard"));
        assert_eq!(x.pid, Some(57));
        assert_eq!(x.level, LogLevel::Default);
        assert_eq!(x.message, "Hello there");
        assert!(!serde_json::to_string(&x).expect("json").contains("MyPhone"));
        let y = entry_from_syslog("Oct  1 09:00:00 MyPhone kernel[0] <Error>: boom", now);
        assert_eq!((y.process.as_deref(), y.pid, y.level), (Some("kernel"), Some(0), LogLevel::Error));
        let z = entry_from_syslog("garbage", now);
        assert_eq!(z.message, "garbage");
    }

    #[test]
    fn filters_apply_on_the_host() {
        let f = LogFilter { min_level: Some(LogLevel::Error), contains: Some("BOOM".into()), ..Default::default() };
        assert!(f.matches(&e(LogLevel::Fault, "a boom happened")));
        assert!(!f.matches(&e(LogLevel::Default, "a boom happened")));
        assert!(!f.matches(&e(LogLevel::Error, "quiet")));
        let f = LogFilter { subsystem: Some("dev.russet".into()), pid: Some(932), ..Default::default() };
        assert!(f.matches(&e(LogLevel::Debug, "x")));
        let f = LogFilter { pid: Some(1), ..Default::default() };
        assert!(!f.matches(&e(LogLevel::Debug, "x")));
    }

    #[tokio::test(start_paused = true)]
    async fn batching_collects_a_window_and_drops_the_oldest() {
        let b = Batcher::new(3);
        for i in 0..5 {
            b.push(e(LogLevel::Default, &format!("m{i}")));
        }
        let batch = b.next(BATCH_WINDOW).await.expect("batch");
        assert_eq!(batch.dropped, 2);
        assert_eq!(batch.entries.iter().map(|x| x.message.as_str()).collect::<Vec<_>>(), vec!["m2", "m3", "m4"]);
        // Entries arriving inside the window join the same batch.
        let b = Arc::new(Batcher::new(10));
        b.push(e(LogLevel::Default, "first"));
        let b2 = Arc::clone(&b);
        let later = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            b2.push(e(LogLevel::Default, "second"));
        });
        let batch = b.next(BATCH_WINDOW).await.expect("batch");
        later.await.expect("join");
        assert_eq!(batch.entries.len(), 2);
        b.close();
        assert_eq!(b.next(BATCH_WINDOW).await, None);
    }

    #[test]
    fn tails_filter_then_keep_the_last_lines() {
        let ring: Vec<(u64, LogEntry)> = (1..=10)
            .map(|i| (i, e(if i % 2 == 0 { LogLevel::Error } else { LogLevel::Info }, &format!("m{i}"))))
            .collect();
        let r =
            select_tail(&ring, &TailQuery { lines: Some(2), min_level: Some(LogLevel::Error), ..Default::default() });
        assert_eq!(r.entries.iter().map(|x| x.message.as_str()).collect::<Vec<_>>(), vec!["m8", "m10"]);
        assert!(r.truncated);
        assert_eq!(r.cursor.as_deref(), Some("10"));
        let r = select_tail(&ring, &TailQuery { cursor: Some("8".into()), ..Default::default() });
        assert_eq!(r.entries.len(), 2);
        assert!(!r.truncated);
    }

    #[test]
    fn timestamps_have_milliseconds() {
        let t = OffsetDateTime::from_unix_timestamp_nanos(1_791_709_200_123_456_789).expect("time");
        assert_eq!(rfc3339_millis(t), "2026-10-11T09:00:00.123Z");
    }
}
