// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Logging (core §12; CORE-13): the daemon's daily files, the reloadable filter of `daemon.setLogFilter`,
//! the records of `daemon.logs.subscribe`, and the redaction that every path goes through.
//!
//! [`RedactingLayer`] formats events itself instead of using `tracing-subscriber`'s formatters, so no field
//! reaches a file, stderr or a subscriber unredacted: values of sensitive field names (`token`, `password`,
//! `authorization`, …) become `***`, and JWTs, PEM blocks, `Bearer` credentials and sensitive `name=value`
//! pairs are masked in messages and values ([`russet_core::redact`]). Span fields (`conn`, `req`, `method`,
//! `job`, …) are appended to their events.
//!
//! - Daemon: `state/logs/russetd.YYYY-MM-DD.log`, `log-retention-days` files (7), text or JSON
//!   (`RUSSET_LOG_FORMAT=json`), stderr too in the foreground. Default filter
//!   `info,idevice=warn,jktcp=warn,hyper=warn`, `RUSSET_LOG` overrides it.
//! - CLI: stderr at `-v`/`-vv`/`-vvv` ([`cli_subscriber`]).
//!
//! journald output (core §12, under systemd) is deferred: `tracing-journald` formats fields itself and would
//! bypass the redaction.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result};
use russet_core::redact::{MASK, is_sensitive_name, redact_text};
use russet_protocol::daemon::{DaemonLogRecord, TraceLevel};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer, Registry, reload};

/// The daemon's default filter.
pub const DEFAULT_FILTER: &str = "info,idevice=warn,jktcp=warn,hyper=warn";

/// How many records `daemon.logs.subscribe` subscribers may lag behind before they lose the oldest.
const RECORD_CHANNEL: usize = 4096;

type Reloader = Box<dyn Fn(EnvFilter) -> Result<()> + Send + Sync>;

/// The live logging controls: the filter and the record stream.
pub struct LogControl {
    reload: Option<Reloader>,
    current: Mutex<String>,
    records: broadcast::Sender<Arc<DaemonLogRecord>>,
}

impl std::fmt::Debug for LogControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogControl")
            .field("filter", &self.filter())
            .field("reloadable", &self.reload.is_some())
            .finish()
    }
}

impl LogControl {
    /// Controls without a global subscriber (an in-process daemon): the filter is only recorded.
    pub fn detached(filter: &str) -> Arc<Self> {
        let (records, _) = broadcast::channel(RECORD_CHANNEL);
        Arc::new(Self { reload: None, current: Mutex::new(filter.to_owned()), records })
    }

    /// The current filter directive.
    pub fn filter(&self) -> String {
        self.current.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Replaces the filter (`daemon.setLogFilter`) and returns the previous one.
    pub fn set_filter(&self, filter: &str) -> Result<String> {
        let parsed = EnvFilter::try_new(filter).map_err(|e| {
            Error::invalid_argument(format!("`{filter}` is not a log filter: {e}"))
                .with_hint("Use EnvFilter syntax, for example `info,russetd=debug`.")
        })?;
        if let Some(reload) = &self.reload {
            reload(parsed)?;
        }
        let mut current = self.current.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(std::mem::replace(&mut *current, filter.to_owned()))
    }

    /// Records for `daemon.logs.subscribe` (already redacted).
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<DaemonLogRecord>> {
        self.records.subscribe()
    }

    /// Hands a record to `daemon.logs.subscribe` subscribers (the logging layer does; tests may too).
    pub fn publish(&self, record: DaemonLogRecord) {
        if self.records.receiver_count() > 0 {
            let _ = self.records.send(Arc::new(record));
        }
    }
}

/// How the daemon logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogOptions {
    /// `RUSSET_LOG`, else [`DEFAULT_FILTER`].
    pub filter: Option<String>,
    /// `RUSSET_LOG_FORMAT=json`.
    pub json: bool,
    /// Also to stderr (`daemon run` in the foreground).
    pub stderr: bool,
    /// Daily files to keep (`daemon.log-retention-days`, 7).
    pub retention_days: usize,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self { filter: None, json: false, stderr: false, retention_days: 7 }
    }
}

impl LogOptions {
    /// `RUSSET_LOG` and `RUSSET_LOG_FORMAT`.
    pub fn from_env() -> Self {
        Self {
            filter: std::env::var("RUSSET_LOG").ok().filter(|v| !v.trim().is_empty()),
            json: std::env::var("RUSSET_LOG_FORMAT").is_ok_and(|v| v.eq_ignore_ascii_case("json")),
            ..Self::default()
        }
    }
}

/// Keeps the file writer flushing; dropping it flushes the last lines.
#[derive(Debug)]
pub struct LogGuard {
    _file: tracing_appender::non_blocking::WorkerGuard,
}

/// Installs the daemon's global subscriber: the reloadable filter, the daily files under `state/logs` and,
/// in the foreground, stderr. Call once per process (`daemon run`); in-process daemons use
/// [`LogControl::detached`].
pub fn init_daemon(dirs: &RussetDirs, opts: &LogOptions) -> Result<(Arc<LogControl>, LogGuard)> {
    let dir = dirs.logs();
    russet_core::fsx::ensure_private_dir(&dir)?;
    sweep_old_logs(&dir, opts.retention_days, std::time::SystemTime::now());
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("russetd")
        .filename_suffix("log")
        .max_log_files(opts.retention_days.max(1))
        .build(&dir)
        .map_err(|e| {
            Error::new(russet_core::error::codes::core::IO, format!("opening the log in {}: {e}", dir.display()))
        })?;
    let (file_writer, guard) = tracing_appender::non_blocking(appender);

    let filter_text = opts.filter.clone().unwrap_or_else(|| DEFAULT_FILTER.to_owned());
    let filter = EnvFilter::try_new(&filter_text).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let (filter_layer, handle) = reload::Layer::new(filter);
    let (records, _) = broadcast::channel(RECORD_CHANNEL);
    let control = Arc::new(LogControl {
        reload: Some(Box::new(move |f: EnvFilter| {
            handle.reload(f).map_err(|e| Error::internal(format!("reloading the log filter: {e}")))
        })),
        current: Mutex::new(filter_text),
        records,
    });
    let file_layer = RedactingLayer::new(file_writer, opts.json).publishing(Arc::clone(&control));
    let stderr_layer = opts.stderr.then(|| RedactingLayer::new(std::io::stderr, opts.json));
    let subscriber = Registry::default().with(filter_layer).with(file_layer).with(stderr_layer);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| Error::internal(format!("a global log subscriber is already installed: {e}")))?;
    Ok((control, LogGuard { _file: guard }))
}

/// The CLI's subscriber: redacted diagnostics on stderr at `-v` (info), `-vv` (debug), `-vvv` (trace);
/// warnings and errors otherwise. `RUSSET_LOG` overrides the level.
pub fn cli_subscriber(verbose: u8) -> impl Subscriber + Send + Sync {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = std::env::var("RUSSET_LOG")
        .ok()
        .and_then(|f| EnvFilter::try_new(f).ok())
        .unwrap_or_else(|| EnvFilter::new(level));
    Registry::default().with(filter).with(RedactingLayer::new(std::io::stderr, false))
}

/// Deletes `russetd.YYYY-MM-DD.log` files older than `retention_days` (by the date in their name).
pub fn sweep_old_logs(dir: &Path, retention_days: usize, now: std::time::SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let cutoff = now - std::time::Duration::from_secs(86_400 * retention_days.max(1) as u64);
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(date) = name.strip_prefix("russetd.").and_then(|r| r.strip_suffix(".log")) else { continue };
        let Some(day) = russet_core::time::parse_rfc3339(&format!("{date}T23:59:59Z")) else { continue };
        if day < cutoff && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// A `tracing` layer that formats every event itself, redacted, as text or JSON (module docs).
pub struct RedactingLayer<W> {
    writer: W,
    json: bool,
    publish: Option<Arc<LogControl>>,
}

impl<W> RedactingLayer<W> {
    /// A layer writing to `writer` (a `MakeWriter`).
    pub fn new(writer: W, json: bool) -> Self {
        Self { writer, json, publish: None }
    }

    /// Also hands every record to `daemon.logs.subscribe` subscribers.
    pub fn publishing(mut self, control: Arc<LogControl>) -> Self {
        self.publish = Some(control);
        self
    }
}

/// A span's recorded fields.
struct SpanFields(Vec<(String, serde_json::Value)>);

#[derive(Default)]
struct Collector {
    message: String,
    fields: Vec<(String, serde_json::Value)>,
}

impl Collector {
    fn push(&mut self, field: &Field, value: serde_json::Value) {
        if field.name() == "message" {
            self.message = match value {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
        } else {
            self.fields.push((field.name().to_owned(), value));
        }
    }
}

impl Visit for Collector {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.push(field, serde_json::Value::String(format!("{value:?}")));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field, serde_json::Value::String(value.to_owned()));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field, serde_json::json!(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field, serde_json::json!(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field, serde_json::json!(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push(field, serde_json::json!(value));
    }
}

fn redact_field(name: &str, value: serde_json::Value) -> serde_json::Value {
    if is_sensitive_name(name) {
        return serde_json::Value::String(MASK.to_owned());
    }
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(redact_text(&s).into_owned()),
        other => other,
    }
}

fn level_of(level: &Level) -> TraceLevel {
    match *level {
        Level::TRACE => TraceLevel::Trace,
        Level::DEBUG => TraceLevel::Debug,
        Level::INFO => TraceLevel::Info,
        Level::WARN => TraceLevel::Warn,
        _ => TraceLevel::Error,
    }
}

/// The redacted record of `event` within the spans of `ctx`.
fn record_of<S: Subscriber + for<'a> LookupSpan<'a>>(event: &Event<'_>, ctx: &Context<'_, S>) -> DaemonLogRecord {
    let meta = event.metadata();
    let mut c = Collector::default();
    event.record(&mut c);
    let mut fields = BTreeMap::new();
    if let Some(scope) = ctx.event_scope(event) {
        for span in scope.from_root() {
            if let Some(sf) = span.extensions().get::<SpanFields>() {
                for (k, v) in &sf.0 {
                    fields.insert(k.clone(), v.clone());
                }
            }
        }
    }
    for (k, v) in c.fields {
        let red = redact_field(&k, v);
        fields.insert(k, red);
    }
    DaemonLogRecord {
        ts: russet_core::time::now_rfc3339(),
        level: level_of(meta.level()),
        target: meta.target().to_owned(),
        message: redact_text(&c.message).into_owned(),
        fields,
    }
}

/// The text form: `2026-10-11T09:00:00.123Z  INFO russetd::jobs: message key=value`.
pub fn format_text(r: &DaemonLogRecord) -> String {
    let level = match r.level {
        TraceLevel::Trace => "TRACE",
        TraceLevel::Debug => "DEBUG",
        TraceLevel::Info => "INFO",
        TraceLevel::Warn => "WARN",
        TraceLevel::Error => "ERROR",
    };
    let mut line = format!("{} {level:>5} {}: {}", r.ts, r.target, r.message);
    for (k, v) in &r.fields {
        match v {
            serde_json::Value::String(s) => line.push_str(&format!(" {k}={s}")),
            other => line.push_str(&format!(" {k}={other}")),
        }
    }
    line
}

impl<S, W> Layer<S> for RedactingLayer<W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'w> tracing_subscriber::fmt::MakeWriter<'w> + 'static,
{
    fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, id: &tracing::span::Id, ctx: Context<'_, S>) {
        let mut c = Collector::default();
        attrs.record(&mut c);
        if let Some(span) = ctx.span(id) {
            let fields = c.fields.into_iter().map(|(k, v)| {
                let red = redact_field(&k, v);
                (k, red)
            });
            span.extensions_mut().insert(SpanFields(fields.collect()));
        }
    }

    fn on_record(&self, id: &tracing::span::Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
        let mut c = Collector::default();
        values.record(&mut c);
        if let Some(span) = ctx.span(id)
            && let Some(sf) = span.extensions_mut().get_mut::<SpanFields>()
        {
            for (k, v) in c.fields {
                let red = redact_field(&k, v);
                sf.0.push((k, red));
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let record = record_of(event, &ctx);
        let line = if self.json { serde_json::to_string(&record).unwrap_or_default() } else { format_text(&record) };
        let mut w = self.writer.make_writer();
        let _ = writeln!(w, "{line}");
        if let Some(control) = &self.publish {
            control.publish(record);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    const CANARY: &str = "RuSsEtLoGcAnArY4d2e";

    #[derive(Clone, Default)]
    struct Buffer(Arc<StdMutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
        type Writer = Buffer;
        fn make_writer(&'a self) -> Buffer {
            self.clone()
        }
    }

    /// Feeds the canary through every shape a secret can take in a log.
    fn emit_canaries() {
        let span = tracing::info_span!("rpc", conn = "c-1", token = CANARY);
        let _g = span.enter();
        tracing::info!(password = CANARY, "login");
        tracing::info!(note = %format!("Authorization: Bearer {CANARY}"), "headers");
        tracing::warn!("asc jwt eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJ{CANARY}.c2ln");
        tracing::error!(blob = %format!("-----BEGIN PRIVATE KEY-----\n{CANARY}\n-----END PRIVATE KEY-----"), "key");
        tracing::info!("url ?token={CANARY}&x=1");
        tracing::debug!(apiKey = CANARY, ok = true, n = 3, "struct");
    }

    #[test]
    fn the_redaction_canary_never_appears_in_text_json_or_records() {
        for json in [false, true] {
            let buf = Buffer::default();
            let control = LogControl::detached("trace");
            let mut rx = control.subscribe();
            let layer = RedactingLayer::new(buf.clone(), json).publishing(Arc::clone(&control));
            let subscriber = Registry::default().with(EnvFilter::new("trace")).with(layer);
            tracing::subscriber::with_default(subscriber, emit_canaries);
            let text = String::from_utf8(buf.0.lock().expect("lock").clone()).expect("utf-8");
            assert_eq!(text.lines().count(), 6, "{text}");
            assert!(!text.contains(CANARY), "canary in the {} output:\n{text}", if json { "JSON" } else { "text" });
            assert!(text.contains("conn"), "span fields are kept: {text}");
            let mut records = 0;
            while let Ok(r) = rx.try_recv() {
                records += 1;
                let as_json = serde_json::to_string(&*r).expect("serializes");
                assert!(!as_json.contains(CANARY), "canary in a subscriber record: {as_json}");
            }
            assert_eq!(records, 6);
        }
    }

    #[test]
    fn filters_reload_and_validate() {
        let control = LogControl::detached("info");
        assert_eq!(control.set_filter("debug,russetd=trace").expect("valid"), "info");
        assert_eq!(control.filter(), "debug,russetd=trace");
        assert!(control.set_filter("not a [filter").is_err());
    }

    #[test]
    fn old_daily_files_are_swept() {
        let tmp = russet_core::fsx::TempDir::new("log-sweep").expect("tempdir");
        let now = russet_core::time::parse_rfc3339("2026-10-11T12:00:00Z").expect("time");
        for name in ["russetd.2026-10-11.log", "russetd.2026-10-05.log", "russetd.2026-09-30.log", "other.log"] {
            std::fs::write(tmp.path().join(name), b"x").expect("write");
        }
        assert_eq!(sweep_old_logs(tmp.path(), 7, now), 1);
        assert!(tmp.path().join("russetd.2026-10-05.log").exists());
        assert!(!tmp.path().join("russetd.2026-09-30.log").exists());
        assert!(tmp.path().join("other.log").exists());
    }
}
