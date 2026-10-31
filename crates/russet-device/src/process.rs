// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Launch, signals, process list, console and exit detection (DEV-07; device.md §6.5).
//!
//! The default launch path is CoreDevice `launchapplication` with `startStopped` and an `openstdiosocket` UUID
//! for stdin, stdout and stderr; the appservice connection stays open as long as the [`LaunchedProcess`] lives (the
//! debug plane attaches while it is open, ARCHITECTURE §17.1). DVT `processcontrol` is the fallback when
//! CoreDevice is unavailable or fails unexpectedly. Exit detection uses the console's end and a `listprocesses`
//! poll; the exit status stays unknown until `monitorprocesstermination` lands (DEV-07b).

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use idevice::ReadWrite;
use idevice::dvt::process_control::ProcessControlClient;
use idevice::dvt::remote_server::RemoteServerClient;
use russet_core::Result;
use russet_core::error::codes;
use russet_protocol::common::{LogEntry, LogLevel, LogSource};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt, WriteHalf};
use tokio::sync::{broadcast, watch};

use crate::coredevice::{
    CoreDeviceClient, InvokeError, LaunchInput, feature, parse_launch, parse_processes, service, signal_input,
};
use crate::error::{NsErrorChain, device_error, from_idevice};
use crate::logs::rfc3339_millis;
use crate::manager::DeviceHandle;
use crate::model::Tri;

/// How to launch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LaunchVia {
    /// CoreDevice, then DVT when CoreDevice is unavailable.
    #[default]
    Auto,
    CoreDevice,
    Dvt,
}

/// A launch request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchRequest {
    pub bundle_id: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub suspended: bool,
    pub kill_existing: bool,
    /// Attach CoreDevice stdio (`openstdiosocket`).
    pub console: bool,
    pub via: LaunchVia,
}

/// How a process ended. The status stays unknown until DEV-07b (`monitorprocesstermination`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub at: OffsetDateTime,
    /// What noticed the exit: `console` (stdio closed) or `poll` (absent from `listprocesses`).
    pub detected_by: &'static str,
}

/// Resolves when the process is gone.
#[derive(Debug, Clone)]
pub struct ExitWatcher {
    rx: watch::Receiver<Option<ExitInfo>>,
}

/// Reports a process's exit to its [`ExitWatcher`]s; the first report wins.
#[derive(Debug, Clone)]
pub struct ExitReporter {
    tx: Arc<watch::Sender<Option<ExitInfo>>>,
}

impl ExitReporter {
    /// Records the exit; `false` when one was already recorded.
    pub fn report(&self, info: ExitInfo) -> bool {
        self.tx.send_if_modified(|e| {
            if e.is_none() {
                *e = Some(info);
                true
            } else {
                false
            }
        })
    }
}

/// A connected exit reporter and watcher (fake devices and tests; launches make their own).
pub fn exit_channel() -> (ExitReporter, ExitWatcher) {
    let (tx, rx) = watch::channel(None);
    (ExitReporter { tx: Arc::new(tx) }, ExitWatcher { rx })
}

impl ExitWatcher {
    /// Waits for the exit.
    pub async fn wait(&mut self) -> ExitInfo {
        loop {
            if let Some(e) = self.rx.borrow().clone() {
                return e;
            }
            if self.rx.changed().await.is_err() {
                return self.rx.borrow().clone().unwrap_or(ExitInfo {
                    code: None,
                    signal: None,
                    at: OffsetDateTime::now_utc(),
                    detected_by: "lost",
                });
            }
        }
    }

    /// The exit, if it already happened.
    pub fn exited(&self) -> Option<ExitInfo> {
        self.rx.borrow().clone()
    }
}

/// How many console lines a late subscriber still gets.
const OUTPUT_BACKLOG: usize = 2000;

/// The console of a launched process: lines as [`LogEntry`] (`source: stdout`) with a backlog, so a subscriber
/// that arrives between a suspended launch and `SIGCONT` misses nothing.
#[derive(Debug)]
pub struct OutputHub {
    pid: u32,
    process: Option<String>,
    bundle_id: Option<String>,
    tx: broadcast::Sender<LogEntry>,
    backlog: Mutex<VecDeque<LogEntry>>,
    finished: AtomicBool,
    lines: AtomicU64,
}

impl OutputHub {
    pub fn new(pid: u32, process: Option<String>) -> Self {
        Self::for_app(pid, process, None)
    }

    /// The console of `bundle_id`'s process `pid`.
    pub fn for_app(pid: u32, process: Option<String>, bundle_id: Option<String>) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            pid,
            process,
            bundle_id,
            tx,
            backlog: Mutex::new(VecDeque::new()),
            finished: AtomicBool::new(false),
            lines: AtomicU64::new(0),
        }
    }

    /// Publishes one console line.
    pub fn push_line(&self, line: &str) {
        let entry = LogEntry {
            ts: rfc3339_millis(OffsetDateTime::now_utc()),
            source: LogSource::Stdout,
            level: LogLevel::Default,
            message: line.to_owned(),
            pid: Some(self.pid),
            tid: None,
            process: self.process.clone(),
            subsystem: None,
            category: None,
            image_uuid: None,
            image_offset: None,
            mach_ts: None,
        };
        if let Ok(mut b) = self.backlog.lock() {
            if b.len() == OUTPUT_BACKLOG {
                b.pop_front();
            }
            b.push_back(entry.clone());
        }
        self.lines.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(entry);
    }

    /// The backlog and a live receiver (no gap between them).
    pub fn subscribe(&self) -> (Vec<LogEntry>, broadcast::Receiver<LogEntry>) {
        self.subscribe_with(usize::MAX)
    }

    /// At most the last `backlog` lines and a live receiver (no gap between them).
    pub fn subscribe_with(&self, backlog: usize) -> (Vec<LogEntry>, broadcast::Receiver<LogEntry>) {
        let guard = self.backlog.lock();
        let rx = self.tx.subscribe();
        let lines = guard
            .map(|b| {
                let skip = b.len().saturating_sub(backlog);
                b.iter().skip(skip).cloned().collect()
            })
            .unwrap_or_default();
        (lines, rx)
    }

    /// The app this console belongs to, when Russet launched it by bundle id.
    pub fn bundle_id(&self) -> Option<&str> {
        self.bundle_id.as_deref()
    }

    /// The process name its lines carry.
    pub fn process(&self) -> Option<&str> {
        self.process.as_deref()
    }

    pub fn finish(&self) {
        self.finished.store(true, Ordering::SeqCst);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// Console lines seen so far.
    pub fn line_count(&self) -> u64 {
        self.lines.load(Ordering::Relaxed)
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
}

/// Splits console bytes into lines (PTY output ends lines in `\r\n`).
#[derive(Debug, Default)]
pub struct LineSplitter {
    buf: Vec<u8>,
}

impl LineSplitter {
    /// Feeds bytes; returns the completed lines.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(String::from_utf8_lossy(&line).into_owned());
        }
        // A runaway line without a newline is flushed at 64 KiB.
        if self.buf.len() > 64 * 1024 {
            out.push(String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned());
        }
        out
    }

    /// The trailing partial line at EOF.
    pub fn finish(&mut self) -> Option<String> {
        (!self.buf.is_empty()).then(|| String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned())
    }
}

/// A launched process; the launch channel stays open until this is dropped (ARCHITECTURE §17.1).
pub struct LaunchedProcess {
    pub pid: u32,
    /// The device path, from CoreDevice's `executableURL` (empty after a DVT launch).
    pub executable: String,
    /// The path that launched it.
    pub via: LaunchVia,
    /// The console, when launched with `console` over CoreDevice.
    pub output: Option<Arc<OutputHub>>,
    pub exit: ExitWatcher,
    stdin: Option<WriteHalf<Box<dyn ReadWrite>>>,
    /// The appservice connection (or the DVT client) that launched the process.
    channel: Option<Box<dyn std::any::Any + Send + Sync>>,
    stop: tokio_util::sync::CancellationToken,
}

impl std::fmt::Debug for LaunchedProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchedProcess")
            .field("pid", &self.pid)
            .field("executable", &self.executable)
            .field("via", &self.via)
            .field("console", &self.output.is_some())
            .field("channel_open", &self.channel.is_some())
            .finish()
    }
}

impl LaunchedProcess {
    /// A launched process without a device behind it, for fake devices and tests: no stdin, no launch channel.
    pub fn from_parts(
        pid: u32,
        executable: String,
        via: LaunchVia,
        output: Option<Arc<OutputHub>>,
        exit: ExitWatcher,
    ) -> Self {
        Self {
            pid,
            executable,
            via,
            output,
            exit,
            stdin: None,
            channel: None,
            stop: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Writes to the app's stdin (CoreDevice console only).
    pub async fn write_stdin(&mut self, bytes: &[u8]) -> Result<()> {
        let w = self.stdin.as_mut().ok_or_else(|| {
            device_error(codes::device::UNSUPPORTED, "this launch has no console (launch with `console`)")
        })?;
        w.write_all(bytes).await.map_err(|e| device_error(codes::device::IO, format!("stdin: {e}")))?;
        w.flush().await.map_err(|e| device_error(codes::device::IO, format!("stdin: {e}")))
    }

    /// Closes the launch channel (appservice connection) while keeping the exit watcher (device check D2 looks at
    /// what the app does then).
    pub fn close_channel(&mut self) {
        self.channel = None;
    }

    /// True while the launch channel is open.
    pub fn channel_open(&self) -> bool {
        self.channel.is_some()
    }
}

impl Drop for LaunchedProcess {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

fn process_name(executable: &str) -> Option<String> {
    executable.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_owned)
}

async fn app_service(h: &DeviceHandle) -> Result<CoreDeviceClient> {
    let stream = h.conn().rsd_service(h.udid(), service::APP_SERVICE).await?;
    CoreDeviceClient::new(stream).await
}

/// True when a CoreDevice failure should fall back to DVT (missing service, transport problems), not when the
/// device answered with a definite reason (locked, not installed, denied).
fn falls_back(e: &russet_core::Error) -> bool {
    let c = e.code();
    c == codes::device::SERVICE_UNAVAILABLE
        || c == codes::device::SERVICE_NOT_IMPLEMENTED
        || c == codes::device::TUNNEL_FAILED
        || c == codes::device::PROTOCOL
}

/// `app.launch` (device.md §6.5).
pub async fn launch(h: &DeviceHandle, r: LaunchRequest) -> Result<LaunchedProcess> {
    if r.bundle_id.is_empty() {
        return Err(crate::error::invalid_argument("a bundle id is required"));
    }
    let res = match r.via {
        LaunchVia::Dvt => launch_dvt(h, &r).await,
        LaunchVia::CoreDevice => launch_coredevice(h, &r).await,
        LaunchVia::Auto => match launch_coredevice(h, &r).await {
            Ok(p) => Ok(p),
            Err(e) if falls_back(&e) => {
                tracing::info!(error = %e.message(), "CoreDevice launch failed; falling back to DVT");
                launch_dvt(h, &r).await.map_err(|dvt| {
                    tracing::debug!(error = %dvt.message(), "DVT launch failed too");
                    e
                })
            }
            Err(e) => Err(e),
        },
    };
    match res {
        Ok(p) => {
            h.set_locked(Tri::No);
            if let Some(out) = &p.output {
                h.actor.register_output(p.pid, Arc::clone(out));
            }
            Ok(p)
        }
        Err(e) => Err(h.note_error(e)),
    }
}

async fn launch_coredevice(h: &DeviceHandle, r: &LaunchRequest) -> Result<LaunchedProcess> {
    // The stdio socket comes first: its UUID goes into the launch request.
    let mut stdio: Option<(Box<dyn ReadWrite>, uuid::Uuid)> = None;
    if r.console {
        let mut s = h.conn().rsd_service(h.udid(), service::OPEN_STDIO_SOCKET).await?;
        let mut id = [0u8; 16];
        s.read_exact(&mut id).await.map_err(|e| device_error(codes::device::PROTOCOL, format!("stdio socket: {e}")))?;
        stdio = Some((s, uuid::Uuid::from_bytes(id)));
    }
    let mut client = app_service(h).await?;
    let input = LaunchInput {
        bundle_id: r.bundle_id.clone(),
        args: r.args.clone(),
        env: r.env.clone(),
        start_suspended: r.suspended,
        terminate_existing: r.kill_existing,
        stdio: stdio.as_ref().map(|(_, u)| *u),
    };
    let out = client.invoke(feature::LAUNCH_APPLICATION, input.to_xpc()).await.map_err(|e| match e {
        InvokeError::Device(v) => NsErrorChain::from_plist(&v).to_error(&format!("launching {}", r.bundle_id)),
        InvokeError::Transport(e) => e,
    })?;
    let launched = parse_launch(&out)?;
    let (exit_tx, exit_rx) = watch::channel(None);
    let stop = tokio_util::sync::CancellationToken::new();
    let name = process_name(&launched.executable);
    let mut output = None;
    let mut stdin = None;
    if let Some((stream, _)) = stdio {
        let hub = Arc::new(OutputHub::for_app(launched.pid, name.clone(), Some(r.bundle_id.clone())));
        let (mut read, write) = tokio::io::split(stream);
        stdin = Some(write);
        let hub2 = Arc::clone(&hub);
        let exit_tx2 = exit_tx.clone();
        tokio::spawn(async move {
            let mut split = LineSplitter::default();
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match read.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for line in split.feed(&buf[..n]) {
                            hub2.push_line(&line);
                        }
                    }
                }
            }
            if let Some(rest) = split.finish() {
                hub2.push_line(&rest);
            }
            hub2.finish();
            // jktcp reports the device's FIN as "channel closed": the process is gone (device.md §6.5).
            exit_tx2.send_if_modified(|e| {
                if e.is_none() {
                    *e = Some(ExitInfo {
                        code: None,
                        signal: None,
                        at: OffsetDateTime::now_utc(),
                        detected_by: "console",
                    });
                    true
                } else {
                    false
                }
            });
        });
        output = Some(hub);
    }
    spawn_exit_poll(
        h.clone(),
        launched.pid,
        exit_tx,
        stop.clone(),
        if r.console { Duration::from_secs(2) } else { Duration::from_secs(1) },
    );
    Ok(LaunchedProcess {
        pid: launched.pid,
        executable: launched.executable,
        via: LaunchVia::CoreDevice,
        output,
        exit: ExitWatcher { rx: exit_rx },
        stdin,
        channel: Some(Box::new(Mutex::new(client))),
        stop,
    })
}

async fn launch_dvt(h: &DeviceHandle, r: &LaunchRequest) -> Result<LaunchedProcess> {
    let stream = h.conn().rsd_service(h.udid(), service::DT_SERVICE_HUB).await?;
    let mut server = RemoteServerClient::new(stream);
    // The hub greets first (device.md Appendix B: the spike reads the hello before opening channels).
    server.read_message(0).await.map_err(|e| from_idevice(e, "DVT hello"))?;
    let mut env = plist::Dictionary::new();
    for (k, v) in &r.env {
        env.insert(k.clone(), v.clone().into());
    }
    let mut args = plist::Dictionary::new();
    for (i, a) in r.args.iter().enumerate() {
        args.insert(i.to_string(), a.clone().into());
    }
    let pid = {
        let mut pc = ProcessControlClient::new(&mut server).await.map_err(|e| from_idevice(e, "DVT processcontrol"))?;
        pc.launch_app(r.bundle_id.clone(), Some(env), Some(args), r.suspended, r.kill_existing)
            .await
            .map_err(|e| from_idevice(e, &format!("DVT launch of {}", r.bundle_id)))?
    };
    let pid = u32::try_from(pid).map_err(|_| device_error(codes::device::PROTOCOL, "DVT returned an invalid pid"))?;
    let (exit_tx, exit_rx) = watch::channel(None);
    let stop = tokio_util::sync::CancellationToken::new();
    spawn_exit_poll(h.clone(), pid, exit_tx, stop.clone(), Duration::from_secs(1));
    Ok(LaunchedProcess {
        pid,
        executable: String::new(),
        via: LaunchVia::Dvt,
        output: None,
        exit: ExitWatcher { rx: exit_rx },
        stdin: None,
        channel: Some(Box::new(Mutex::new(server))),
        stop,
    })
}

/// Polls `listprocesses` until the pid is gone, the exit was seen elsewhere, or the launch handle is dropped.
fn spawn_exit_poll(
    h: DeviceHandle,
    pid: u32,
    tx: watch::Sender<Option<ExitInfo>>,
    stop: tokio_util::sync::CancellationToken,
    every: Duration,
) {
    tokio::spawn(async move {
        let mut client: Option<CoreDeviceClient> = None;
        loop {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tx.closed() => return,
                _ = tokio::time::sleep(every) => {}
            }
            if tx.borrow().is_some() {
                return;
            }
            if client.is_none() {
                client = app_service(&h).await.ok();
            }
            let Some(c) = client.as_mut() else { continue };
            match c.invoke(feature::LIST_PROCESSES, idevice::xpc::Dictionary::new()).await {
                Ok(out) => {
                    if let Ok(list) = parse_processes(&out)
                        && !list.iter().any(|p| p.pid == pid)
                    {
                        tx.send_if_modified(|e| {
                            if e.is_none() {
                                *e = Some(ExitInfo {
                                    code: None,
                                    signal: None,
                                    at: OffsetDateTime::now_utc(),
                                    detected_by: "poll",
                                });
                                true
                            } else {
                                false
                            }
                        });
                        return;
                    }
                }
                Err(_) => client = None,
            }
        }
    });
}

/// `process.signal` (CoreDevice `sendsignaltoprocess`; SIGCONT resumes a suspended launch).
pub async fn signal(h: &DeviceHandle, pid: u32, sig: i32) -> Result<()> {
    if !(1..=31).contains(&sig) {
        return Err(crate::error::invalid_argument(format!("signal {sig} is out of range 1–31")));
    }
    let mut client = app_service(h).await?;
    client.invoke(feature::SEND_SIGNAL, signal_input(pid, sig)).await.map(|_| ()).map_err(|e| match e {
        InvokeError::Device(v) => {
            let chain = NsErrorChain::from_plist(&v);
            let text = chain.to_string().to_ascii_lowercase();
            if text.contains("no such process") || text.contains("not found") || chain.contains("NSPOSIXErrorDomain", 3)
            {
                russet_core::Error::new(codes::core::NOT_FOUND, format!("no process {pid} on the device"))
                    .with_hint("`russet process list` shows the running processes.")
            } else {
                let e = chain.to_error(&format!("signal {sig} to {pid}"));
                if e.code() == codes::app::LAUNCH_FAILED {
                    device_error(codes::device::PROTOCOL, format!("signal {sig} to {pid}: {chain}"))
                } else {
                    e
                }
            }
        }
        InvokeError::Transport(e) => e,
    })
}

/// One process on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    /// The executable's device path.
    pub executable: Option<String>,
    /// For user apps (matched against the installed apps' paths).
    pub bundle_id: Option<String>,
    /// Runs from an `.app` bundle.
    pub is_application: bool,
}

/// `process.list` (CoreDevice `listprocesses`; bundle ids for user apps).
pub async fn list(h: &DeviceHandle) -> Result<Vec<ProcessInfo>> {
    let mut client = app_service(h).await?;
    let out = client
        .invoke(feature::LIST_PROCESSES, idevice::xpc::Dictionary::new())
        .await
        .map_err(|e| e.into_error("listing processes"))?;
    let tokens = parse_processes(&out)?;
    let needs_apps = tokens.iter().any(|t| t.executable.as_deref().is_some_and(|e| e.contains("/Bundle/Application/")));
    let apps = if needs_apps {
        crate::apps::list(h, &crate::apps::AppFilter::default()).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(tokens
        .into_iter()
        .map(|t| {
            let exe = t.executable.clone().unwrap_or_default();
            let bundle_id = apps
                .iter()
                .find(|a| a.path.as_deref().is_some_and(|p| !p.is_empty() && exe.starts_with(&format!("{p}/"))))
                .map(|a| a.bundle_id.clone());
            ProcessInfo {
                pid: t.pid,
                name: process_name(&exe).unwrap_or_default(),
                is_application: exe.contains(".app/"),
                executable: t.executable,
                bundle_id,
            }
        })
        .collect())
}

/// The pids running an app's executable.
pub async fn pids_of(h: &DeviceHandle, bundle_id: &str) -> Result<Vec<u32>> {
    Ok(list(h).await?.into_iter().filter(|p| p.bundle_id.as_deref() == Some(bundle_id)).map(|p| p.pid).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_bytes_split_into_lines() {
        let mut s = LineSplitter::default();
        assert_eq!(s.feed(b"probe-"), Vec::<String>::new());
        assert_eq!(s.feed(b"start\r\nbeat-"), vec!["probe-start".to_owned()]);
        assert_eq!(s.feed(b"1\n\n"), vec!["beat-1".to_owned(), String::new()]);
        assert_eq!(s.feed(b"tail"), Vec::<String>::new());
        assert_eq!(s.finish(), Some("tail".into()));
        assert_eq!(s.finish(), None);
    }

    #[tokio::test]
    async fn output_hubs_keep_a_backlog_for_late_subscribers() {
        let hub = OutputHub::new(932, Some("RussetProbe".into()));
        hub.push_line("before");
        let (backlog, mut rx) = hub.subscribe();
        hub.push_line("after");
        assert_eq!(backlog.len(), 1);
        assert_eq!(backlog[0].message, "before");
        assert_eq!(backlog[0].source, LogSource::Stdout);
        assert_eq!(backlog[0].pid, Some(932));
        assert_eq!(rx.recv().await.expect("live").message, "after");
        assert_eq!(hub.line_count(), 2);
    }

    #[tokio::test]
    async fn exit_watchers_resolve() {
        let (tx, rx) = watch::channel(None);
        let mut w = ExitWatcher { rx };
        assert!(w.exited().is_none());
        tx.send_replace(Some(ExitInfo {
            code: None,
            signal: Some(9),
            at: OffsetDateTime::UNIX_EPOCH,
            detected_by: "poll",
        }));
        assert_eq!(w.wait().await.signal, Some(9));
    }

    #[test]
    fn process_names_come_from_the_executable() {
        assert_eq!(process_name("/private/var/containers/Bundle/Application/X/P.app/P"), Some("P".into()));
        assert_eq!(process_name(""), None);
    }
}
