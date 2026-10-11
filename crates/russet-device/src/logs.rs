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
//! device for `logs.tail`, and stops [`TAIL_IDLE`] after the last `logs.tail` call however busy its source is.
//!
//! A stream for a `bundle_id` follows the app (device.md §6.7): os_trace and the console match the app's executable
//! name, so a relaunch (a new pid) keeps flowing into the same stream. A stream for a `pid` stays with that process.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use idevice::Idevice;
use idevice::os_trace_relay::{OsTraceLog, OsTraceRelayClient};
use idevice::syslog_relay::SyslogRelayClient;
use russet_core::Result;
use russet_protocol::common::{LogEntry, LogLevel, LogSource};
use time::OffsetDateTime;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::actor::DeviceActor;
use crate::error::{device_error, from_idevice, invalid_argument};
use crate::manager::DeviceHandle;
use crate::pairing::LABEL;
use crate::process::OutputHub;

/// The batching window (device.md §6.7).
pub const BATCH_WINDOW: Duration = Duration::from_millis(50);
/// Pending entries per stream before the oldest are dropped (ARCHITECTURE §6.4).
pub const STREAM_CAPACITY: usize = 10_000;
/// The `logs.tail` ring per capture (device.md §6.7).
pub const TAIL_CAPACITY: usize = 20_000;
/// A tail capture stops after this long without a `logs.tail` call.
pub const TAIL_IDLE: Duration = Duration::from_secs(120);
/// How often a capture checks whether it is idle.
pub const TAIL_IDLE_CHECK: Duration = Duration::from_secs(10);

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
    /// Console lines from before the stream opened to deliver first (`logs.subscribe {backlog}`): the `stdout`
    /// source keeps the last 2,000 lines of each process Russet launched. os_trace and syslog have no history on
    /// the device. `0` (the default) delivers only new entries.
    pub backlog: usize,
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

#[cfg(test)]
impl LogStream {
    /// A stream over an existing batcher.
    pub(crate) fn from_batcher(batcher: Arc<Batcher>) -> Self {
        Self { batcher, stop: CancellationToken::new() }
    }
}

/// Counts the readers of a stream; the batcher closes when the last one ends.
#[derive(Clone)]
struct Readers {
    n: Arc<AtomicU64>,
    batcher: Arc<Batcher>,
}

impl Readers {
    fn start(&self) -> ReaderGuard {
        self.n.fetch_add(1, Ordering::SeqCst);
        ReaderGuard(self.clone())
    }
}

struct ReaderGuard(Readers);

impl Drop for ReaderGuard {
    fn drop(&mut self) {
        if self.0.n.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.batcher.close();
        }
    }
}

/// Feeds one console into the stream: at most `backlog` earlier lines, then everything new.
fn spawn_console_reader(
    hub: &OutputHub,
    backlog: usize,
    b: Arc<Batcher>,
    s: CancellationToken,
    filt: LogFilter,
    r: &Readers,
) {
    let (earlier, mut rx) = hub.subscribe_with(backlog);
    let guard = r.start();
    tokio::spawn(async move {
        let _guard = guard;
        for e in earlier {
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
    });
}

/// Opens the sources of `f` and merges them.
///
/// - `pid`: os_trace filtered on the device, and that process's console.
/// - `bundle_id` (no `pid`): the app's executable name on the host, so the stream follows relaunches; the consoles
///   of its running processes and of every later launch. `app.notInstalled` when the app is not on the device.
pub async fn open(h: &DeviceHandle, mut f: LogFilter) -> Result<LogStream> {
    let batcher = Arc::new(Batcher::new(STREAM_CAPACITY));
    let stop = CancellationToken::new();
    let readers = Readers { n: Arc::new(AtomicU64::new(0)), batcher: Arc::clone(&batcher) };
    // The stream does not end while it is being set up.
    let setup = readers.start();
    let mut pids: Vec<u32> = f.pid.into_iter().collect();
    let mut follow: Option<String> = None;
    if f.pid.is_none()
        && let Some(b) = f.bundle_id.clone()
    {
        let app = crate::apps::lookup(h, &b).await?.ok_or_else(|| {
            device_error(russet_core::error::codes::app::NOT_INSTALLED, format!("{b} is not installed"))
        })?;
        match app.executable.filter(|e| !e.is_empty()) {
            Some(exe) => {
                f.process.get_or_insert(exe);
                follow = Some(b);
            }
            // Without an executable name, stay with the running process.
            None => {
                pids = crate::process::pids_of(h, &b).await?;
                f.pid = Some(*pids.first().ok_or_else(|| {
                    russet_core::Error::new(russet_core::error::codes::core::NOT_FOUND, format!("{b} is not running"))
                })?);
            }
        }
    }
    if f.wants(LogSourceKind::OsLog) {
        let stream = h.conn().lockdown_service(h.udid(), "com.apple.os_trace_relay").await?;
        let client = OsTraceRelayClient { idevice: Idevice::new(stream, LABEL) };
        let mut rx = client.start_trace(f.pid).await.map_err(|e| from_idevice(e, "starting os_trace_relay"))?;
        let (b, s, filt, guard) = (Arc::clone(&batcher), stop.clone(), f.clone(), readers.start());
        tokio::spawn(async move {
            let _guard = guard;
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
        });
    }
    if f.wants(LogSourceKind::Syslog) {
        let stream = h.conn().lockdown_service(h.udid(), "com.apple.syslog_relay").await?;
        let mut client = SyslogRelayClient::new(Idevice::new(stream, LABEL));
        let (b, s, filt, guard) = (Arc::clone(&batcher), stop.clone(), f.clone(), readers.start());
        tokio::spawn(async move {
            let _guard = guard;
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
        });
    }
    if f.wants(LogSourceKind::Stdout) {
        match follow {
            Some(bundle) => {
                // Subscribe before listing, so a launch in between is not missed (its pid is deduplicated).
                let mut later = h.actor.output_events.subscribe();
                let now = h.actor.outputs_of_bundle(&bundle);
                let mut seen: BTreeSet<u32> = BTreeSet::new();
                for hub in &now {
                    seen.insert(hub.pid());
                    spawn_console_reader(hub, f.backlog, Arc::clone(&batcher), stop.clone(), f.clone(), &readers);
                }
                let (b, s, filt, r, guard) =
                    (Arc::clone(&batcher), stop.clone(), f.clone(), readers.clone(), readers.start());
                tokio::spawn(async move {
                    let _guard = guard;
                    loop {
                        let hub = tokio::select! {
                            _ = s.cancelled() => break,
                            h = later.recv() => h,
                        };
                        match hub {
                            Ok(hub) if hub.bundle_id() == Some(bundle.as_str()) && seen.insert(hub.pid()) => {
                                // Everything a new launch printed is new to this stream.
                                spawn_console_reader(&hub, usize::MAX, Arc::clone(&b), s.clone(), filt.clone(), &r);
                            }
                            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(_) => break,
                        }
                    }
                });
            }
            None => {
                let hubs: Vec<Arc<OutputHub>> = pids.iter().filter_map(|p| h.actor.output(*p)).collect();
                for hub in hubs {
                    spawn_console_reader(&hub, f.backlog, Arc::clone(&batcher), stop.clone(), f.clone(), &readers);
                }
            }
        }
    }
    drop(setup);
    Ok(LogStream { batcher, stop })
}

/// `logs.tail` ring sequence numbers are process-wide, so a cursor from a capture that ended never hides the
/// entries of the capture that replaced it.
static TAIL_SEQ: AtomicU64 = AtomicU64::new(0);

/// One `logs.tail` ring entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailItem {
    /// The cursor value (process-wide, increasing).
    pub seq: u64,
    /// `entry.ts` in Unix milliseconds (`None` when it does not parse).
    pub at_ms: Option<i64>,
    pub entry: LogEntry,
}

impl TailItem {
    /// Wraps an entry with the next sequence number.
    pub fn new(entry: LogEntry) -> Self {
        let at_ms = OffsetDateTime::parse(&entry.ts, &time::format_description::well_known::Rfc3339)
            .ok()
            .map(|t| (t.unix_timestamp_nanos().div_euclid(1_000_000)) as i64);
        Self { seq: TAIL_SEQ.fetch_add(1, Ordering::SeqCst) + 1, at_ms, entry }
    }
}

/// A background capture feeding the `logs.tail` ring.
#[derive(Debug)]
pub struct LogCapture {
    ring: Mutex<VecDeque<TailItem>>,
    last_used: Mutex<tokio::time::Instant>,
    started: tokio::time::Instant,
    stop: CancellationToken,
}

impl LogCapture {
    fn new() -> Self {
        let now = tokio::time::Instant::now();
        Self {
            ring: Mutex::new(VecDeque::new()),
            last_used: Mutex::new(now),
            started: now,
            stop: CancellationToken::new(),
        }
    }

    fn push(&self, e: LogEntry) {
        let item = TailItem::new(e);
        if let Ok(mut r) = self.ring.lock() {
            if r.len() == TAIL_CAPACITY {
                r.pop_front();
            }
            r.push_back(item);
        }
    }

    fn touch(&self) {
        if let Ok(mut l) = self.last_used.lock() {
            *l = tokio::time::Instant::now();
        }
    }

    fn idle_for(&self, idle: Duration) -> bool {
        self.last_used.lock().map(|l| l.elapsed() >= idle).unwrap_or(true)
    }

    /// Whether the capture still runs.
    pub fn is_running(&self) -> bool {
        !self.stop.is_cancelled()
    }

    /// Stops the capture.
    pub fn stop(&self) {
        self.stop.cancel();
    }
}

/// Runs a capture until it was not used for `idle` (checked every `check`, independently of how often entries
/// arrive), its source ends, or it is stopped; then frees the ring and calls `on_stop`.
async fn run_capture(
    c: Arc<LogCapture>,
    mut stream: LogStream,
    idle: Duration,
    check: Duration,
    on_stop: impl FnOnce(),
) {
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + check, check);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = c.stop.cancelled() => break,
            _ = tick.tick() => {
                if c.idle_for(idle) {
                    tracing::debug!("logs.tail capture idle; stopping it");
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
    stream.close();
    if let Ok(mut r) = c.ring.lock() {
        *r = VecDeque::new();
    }
    on_stop();
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
    /// RFC 3339 (any offset, any fraction); only entries at or after it.
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

/// `since` as Unix milliseconds, rounded up (entries carry milliseconds); `core.invalidArgument` unless RFC 3339.
pub fn parse_since(since: &str) -> Result<i64> {
    let t = OffsetDateTime::parse(since.trim(), &time::format_description::well_known::Rfc3339).map_err(|e| {
        invalid_argument(format!("since {since:?} is not an RFC 3339 time: {e}"))
            .with_hint("Use a time like 2026-10-11T09:00:00Z or 2026-10-11T11:00:00.5+02:00.")
    })?;
    let ns = t.unix_timestamp_nanos();
    let ms = ns.div_euclid(1_000_000) + i128::from(ns.rem_euclid(1_000_000) > 0);
    Ok(ms as i64)
}

/// Picks entries from a ring: filter, then the last `lines`.
pub fn select_tail(ring: &[TailItem], q: &TailQuery) -> Result<TailResult> {
    let lines = q.lines.unwrap_or(200).clamp(1, 2000) as usize;
    let after: u64 = match q.cursor.as_deref() {
        None => 0,
        Some(c) => c.trim().parse().map_err(|_| invalid_argument(format!("cursor {c:?} is not a logs.tail cursor")))?,
    };
    let since = q.since.as_deref().map(parse_since).transpose()?;
    let filter = LogFilter { min_level: q.min_level, contains: q.contains.clone(), ..Default::default() };
    let matched: Vec<&TailItem> = ring
        .iter()
        .filter(|i| {
            i.seq > after && filter.matches(&i.entry) && since.is_none_or(|s| i.at_ms.is_some_and(|at| at >= s))
        })
        .collect();
    let truncated = matched.len() > lines;
    let picked = &matched[matched.len().saturating_sub(lines)..];
    let cursor = picked.last().map(|i| i.seq.to_string()).or_else(|| q.cursor.clone());
    Ok(TailResult { entries: picked.iter().map(|i| i.entry.clone()).collect(), truncated, cursor })
}

/// Removes `capture` from its device's table (if it is still the one under `key`).
fn forget_capture(actor: &Weak<DeviceActor>, key: &str, capture: &Arc<LogCapture>) {
    if let Some(a) = actor.upgrade()
        && let Ok(mut m) = a.captures.lock()
        && m.get(key).is_some_and(|c| Arc::ptr_eq(c, capture))
    {
        m.remove(key);
    }
}

/// `logs.tail`: reads the ring of the matching capture, starting one (and waiting `first_wait`) when none runs.
/// The capture stops [`TAIL_IDLE`] after the last call.
pub async fn tail(h: &DeviceHandle, q: TailQuery) -> Result<TailResult> {
    if let Some(s) = q.since.as_deref() {
        parse_since(s)?;
    }
    let key = match (q.pid, q.bundle_id.as_deref()) {
        (Some(p), _) => format!("pid:{p}"),
        (None, Some(b)) => format!("bundle:{b}"),
        _ => "all".to_owned(),
    };
    let existing = h.actor.captures.lock().ok().and_then(|m| m.get(&key).cloned());
    let capture = match existing {
        Some(c) if c.is_running() => c,
        _ => {
            let filter = LogFilter {
                pid: q.pid,
                bundle_id: q.bundle_id.clone(),
                sources: vec![LogSourceKind::OsLog, LogSourceKind::Stdout],
                // The console's earlier lines belong in the tail (`russet logs --app` right after a launch).
                backlog: usize::MAX,
                ..Default::default()
            };
            let stream = open(h, filter).await?;
            let capture = Arc::new(LogCapture::new());
            let (c, actor, k) = (Arc::clone(&capture), Arc::downgrade(&h.actor), key.clone());
            tokio::spawn(async move {
                let c2 = Arc::clone(&c);
                run_capture(c, stream, TAIL_IDLE, TAIL_IDLE_CHECK, move || forget_capture(&actor, &k, &c2)).await;
            });
            if let Ok(mut m) = h.actor.captures.lock() {
                m.retain(|_, c| c.is_running());
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
    let ring: Vec<TailItem> = capture.ring.lock().map(|r| r.iter().cloned().collect()).unwrap_or_default();
    select_tail(&ring, &q)
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

    fn ring_of(entries: Vec<LogEntry>) -> Vec<TailItem> {
        entries.into_iter().map(TailItem::new).collect()
    }

    #[test]
    fn tails_filter_then_keep_the_last_lines() {
        let ring = ring_of(
            (1..=10).map(|i| e(if i % 2 == 0 { LogLevel::Error } else { LogLevel::Info }, &format!("m{i}"))).collect(),
        );
        let r =
            select_tail(&ring, &TailQuery { lines: Some(2), min_level: Some(LogLevel::Error), ..Default::default() })
                .expect("tail");
        assert_eq!(r.entries.iter().map(|x| x.message.as_str()).collect::<Vec<_>>(), vec!["m8", "m10"]);
        assert!(r.truncated);
        assert_eq!(r.cursor.as_deref(), Some(ring[9].seq.to_string().as_str()));
        let r = select_tail(&ring, &TailQuery { cursor: Some(ring[7].seq.to_string()), ..Default::default() })
            .expect("tail");
        assert_eq!(r.entries.len(), 2);
        assert!(!r.truncated);
        assert_eq!(
            select_tail(&ring, &TailQuery { cursor: Some("soon".into()), ..Default::default() })
                .expect_err("bad cursor")
                .code(),
            russet_core::error::codes::core::INVALID_ARGUMENT
        );
        // Sequence numbers keep growing across rings, so an old cursor never hides a new capture's entries.
        let newer = ring_of(vec![e(LogLevel::Info, "after a restart")]);
        let r = select_tail(&newer, &TailQuery { cursor: Some(ring[9].seq.to_string()), ..Default::default() })
            .expect("tail");
        assert_eq!(r.entries.len(), 1);
    }

    fn at(ts: &str, msg: &str) -> LogEntry {
        LogEntry { ts: ts.into(), ..e(LogLevel::Info, msg) }
    }

    #[test]
    fn since_compares_times_not_strings() {
        let ring = ring_of(vec![
            at("2026-10-11T08:59:59.999Z", "before"),
            at("2026-10-11T09:00:00.000Z", "on the second"),
            at("2026-10-11T09:00:00.123Z", "fraction"),
            at("2026-10-11T09:00:01.000Z", "later"),
        ]);
        let msgs = |since: &str| -> Vec<String> {
            select_tail(&ring, &TailQuery { since: Some(since.into()), ..Default::default() })
                .expect("tail")
                .entries
                .into_iter()
                .map(|x| x.message)
                .collect()
        };
        // Whole seconds: `.123Z` is after `09:00:00Z` (a string compare said '.' < 'Z' and dropped it).
        assert_eq!(msgs("2026-10-11T09:00:00Z"), ["on the second", "fraction", "later"]);
        // Other offsets are the same instant.
        assert_eq!(msgs("2026-10-11T11:00:00+02:00"), ["on the second", "fraction", "later"]);
        assert_eq!(msgs("2026-10-11T04:00:00.123-05:00"), ["fraction", "later"]);
        // Sub-millisecond precision rounds up: entries carry milliseconds.
        assert_eq!(msgs("2026-10-11T09:00:00.1225Z"), ["fraction", "later"]);
        assert_eq!(msgs("2026-10-11T09:00:00.1235Z"), ["later"]);
        assert_eq!(msgs("2026-10-11T08:59:59.9991Z"), ["on the second", "fraction", "later"]);
        for bad in ["yesterday", "2026-10-11 09:00:00", "2026-10-11T09:00:00"] {
            let err = select_tail(&ring, &TailQuery { since: Some(bad.into()), ..Default::default() })
                .expect_err("not RFC 3339");
            assert_eq!(err.code(), russet_core::error::codes::core::INVALID_ARGUMENT, "{bad}");
        }
    }

    /// The review's probe: entries every 100 ms must not keep a capture alive; only `logs.tail` calls do.
    #[tokio::test(start_paused = true)]
    async fn a_capture_stops_when_unused_however_busy_its_source_is() {
        let batcher = Arc::new(Batcher::new(STREAM_CAPACITY));
        let stream = LogStream::from_batcher(Arc::clone(&batcher));
        let capture = Arc::new(LogCapture::new());
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&stopped);
        let task = tokio::spawn(run_capture(Arc::clone(&capture), stream, TAIL_IDLE, TAIL_IDLE_CHECK, move || {
            flag.store(true, Ordering::SeqCst)
        }));
        let producer = {
            let b = Arc::clone(&batcher);
            tokio::spawn(async move {
                let mut i = 0u64;
                loop {
                    b.push(e(LogLevel::Default, &format!("beat {i}")));
                    i += 1;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
        };
        // In use: a `logs.tail` call every 60 s keeps it running.
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_secs(60)).await;
            assert!(capture.is_running());
            capture.touch();
        }
        assert!(!capture.ring.lock().expect("ring").is_empty());
        // Unused: it stops within TAIL_IDLE plus one check, while the producer keeps going.
        let started = tokio::time::Instant::now();
        tokio::time::timeout(TAIL_IDLE + TAIL_IDLE_CHECK + Duration::from_secs(1), task)
            .await
            .expect("the capture stopped")
            .expect("join");
        assert!(started.elapsed() >= TAIL_IDLE - TAIL_IDLE_CHECK);
        assert!(!capture.is_running());
        assert!(stopped.load(Ordering::SeqCst), "on_stop ran");
        assert!(capture.ring.lock().expect("ring").is_empty(), "the ring is freed");
        assert!(!producer.is_finished());
        producer.abort();
    }

    #[test]
    fn a_subscription_gets_at_most_its_backlog() {
        let hub = OutputHub::new(932, Some("RussetProbe".into()));
        for i in 0..5 {
            hub.push_line(&format!("line {i}"));
        }
        assert!(hub.subscribe_with(0).0.is_empty());
        let (two, _) = hub.subscribe_with(2);
        assert_eq!(two.iter().map(|x| x.message.as_str()).collect::<Vec<_>>(), vec!["line 3", "line 4"]);
        assert_eq!(hub.subscribe().0.len(), 5);
    }

    #[test]
    fn timestamps_have_milliseconds() {
        let t = OffsetDateTime::from_unix_timestamp_nanos(1_791_709_200_123_456_789).expect("time");
        assert_eq!(rfc3339_millis(t), "2026-10-11T09:00:00.123Z");
    }
}
