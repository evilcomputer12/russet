// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The gdb-remote broker (RD-03; RD §6; ARCHITECTURE §10.3).
//!
//! Before LLDB connects, the broker owns the session's single debugserver connection:
//! 1. it handshakes upstream (`+`, `QStartNoAckMode`, `qSupported`, `QThreadSuffixSupported`,
//!    `QListThreadsInStopReply`, `QEnableErrorStrings`, `qHostInfo`);
//! 2. it sends `QSetDetachOnError` (0 for Russet-launched apps, 1 for attach-to-running), so the outcome of a
//!    lost connection is explicit (RD §4.6);
//! 3. it sends `vAttach` while the app is still suspended before dyld (or `vRun` for host tests).
//!
//! Then [`Broker::serve`] listens on one local endpoint (a 0600 Unix socket, or loopback TCP) for one LLDB at a
//! time and relays frames with separate ack state per side. LLDB's synchronous `process connect` returns a
//! stopped process at once, so no Python and no async toggling are needed (RD V3–V4). The relay refuses LLDB's
//! own `vAttach`/`vRun`, keeps `QSetDetachOnError` to itself and strips `SupportedCompressions`. LLDB's `D`
//! is forwarded (the "IDE Detach" row of RD §4.6, which `russet debug --detach` uses); a debugger that goes away
//! without `D` or `k` gets the `onDebuggerLost` policy.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use russet_core::Result;
use russet_core::error::codes;
use russet_gdbremote::interpose::{DownstreamAction, InterposeHooks, Interposer, RelayEnd, UpstreamStopAction};
use russet_gdbremote::{GdbClient, GdbError, HandshakeOptions, ServerInfo, StopEvent, StopReply};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::errors;
use crate::{BoxStream, BrokerEndpoint, TerminateMode};

/// The RSD service that starts a debugserver per connection.
pub const DEBUGPROXY_SERVICE: &str = "com.apple.internal.dt.remote.debugproxy";
/// How long the broker waits for its first debugger (RD §6.3, ARCHITECTURE §10.3).
pub const HANDOFF_TIMEOUT: Duration = Duration::from_secs(60);

/// What happens when the debugger goes away without `D` or `k` (RD §4.6 `onDebuggerLost`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostPolicy {
    /// Kill the app (the default for Russet-launched sessions).
    Kill,
    /// Detach; the app keeps running.
    Detach,
    /// Keep the app stopped under the broker and accept another debugger.
    Keep,
}

/// Broker settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerOptions {
    /// `QSetDetachOnError:1` (detach when the broker's own connection drops) instead of `:0` (kill).
    pub detach_on_error: bool,
    pub on_debugger_lost: LostPolicy,
    /// `None` waits for the first debugger forever.
    pub handoff_timeout: Option<Duration>,
    /// Per-request timeout towards debugserver.
    pub request_timeout: Duration,
}

impl Default for BrokerOptions {
    fn default() -> Self {
        Self {
            detach_on_error: false,
            on_debugger_lost: LostPolicy::Kill,
            handoff_timeout: Some(HANDOFF_TIMEOUT),
            request_timeout: Duration::from_secs(30),
        }
    }
}

/// Where to listen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointSpec {
    /// A Unix socket; its directory is created 0700 and the socket is made 0600.
    #[cfg(unix)]
    Unix { path: PathBuf },
    /// Loopback TCP (Windows, and on request); port 0 picks one.
    Tcp { bind: SocketAddr },
}

/// The broker's state (RD §6.6, without the JIT states).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerState {
    /// Attached and waiting for a debugger.
    Ready,
    /// A debugger is connected and frames are relayed.
    Relaying,
    /// The debugger detached (or the broker did); the app keeps running.
    Detached,
    /// The process exited (`W`) or was killed (`X`).
    Exited { code: Option<i32>, signal: Option<i32> },
    /// debugserver's connection went away.
    Lost,
    /// No debugger connected before the handoff deadline; the policy was applied.
    HandoffTimedOut,
}

impl BrokerState {
    /// Whether the broker is done.
    pub fn is_final(self) -> bool {
        !matches!(self, Self::Ready | Self::Relaying)
    }
}

/// Things the session wants to hear about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerEvent {
    DebuggerConnected,
    /// The debugger detached with `D`; the app keeps running.
    DebuggerDetached,
    /// The debugger went away without `D` or `k`; the policy was applied.
    DebuggerLost(LostPolicy),
    /// The process ended; `requested` when the debugger's `k` or the broker itself killed it.
    ProcessExited {
        code: Option<i32>,
        signal: Option<i32>,
        requested: bool,
    },
    /// debugserver's connection went away.
    Lost,
    HandoffTimeout,
    /// The debugger resumed the process.
    Resumed,
    /// The process stopped under the debugger (`reason` from the stop reply, when it has one).
    Stopped {
        reason: Option<String>,
    },
}

/// A pre-attached broker that has not started serving yet.
pub struct Broker {
    client: GdbClient<BoxStream>,
    opts: BrokerOptions,
    pid: Option<u32>,
    attach_stop: Option<StopReply>,
}

impl std::fmt::Debug for Broker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Broker").field("pid", &self.pid).field("opts", &self.opts).finish_non_exhaustive()
    }
}

impl Broker {
    /// Handshakes with debugserver over `stream` and sets `QSetDetachOnError` (RD §6.2 steps 1–4 and 6).
    pub async fn connect(stream: BoxStream, opts: BrokerOptions) -> Result<Self> {
        let hs = HandshakeOptions { no_ack: true, timeout: Some(opts.request_timeout), host_info: true };
        let (mut client, _) = GdbClient::handshake(stream, hs).await.map_err(errors::debugserver_unavailable)?;
        let value = if opts.detach_on_error { 1 } else { 0 };
        // lldb-server does not know the packet (empty reply); debugserver answers OK.
        let reply = client.request(format!("QSetDetachOnError:{value}").as_bytes()).await.map_err(errors::lost)?;
        if reply.first() == Some(&b'E') {
            tracing::warn!(reply = %String::from_utf8_lossy(&reply), "QSetDetachOnError refused");
        }
        Ok(Self { client, opts, pid: None, attach_stop: None })
    }

    /// What the handshake found (page size, OS, compressions).
    pub fn server(&self) -> &ServerInfo {
        self.client.server()
    }

    /// `vAttach;<pid>` while the app is suspended: debugserver owns a stopped process afterwards.
    pub async fn attach(&mut self, pid: u32) -> Result<StopReply> {
        let stop = self.client.attach(pid).await.map_err(|e| errors::attach_failed(pid, e))?;
        self.pid = Some(pid);
        self.attach_stop = Some(stop.clone());
        Ok(stop)
    }

    /// `vRun`: lldb-server launches `program` stopped at its entry (host tests only; debugserver on iOS 17+
    /// cannot spawn apps).
    pub async fn run_program(&mut self, program: &str, args: &[String]) -> Result<StopReply> {
        let stop = self.client.run_program(program, args).await.map_err(|e| errors::attach_failed(0, e))?;
        let pid = self.client.process_info().await.map(|p| p.pid).ok().filter(|p| *p != 0);
        self.pid = pid;
        self.attach_stop = Some(stop.clone());
        Ok(stop)
    }

    /// The attached process.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The stop `vAttach` reported.
    pub fn attach_stop(&self) -> Option<&StopReply> {
        self.attach_stop.as_ref()
    }

    /// Binds `spec` and relays for one debugger at a time until the session ends.
    pub async fn serve(self, spec: EndpointSpec) -> Result<BrokerHandle> {
        let (listener, endpoint) = Listener::bind(&spec).await?;
        let (state_tx, state_rx) = watch::channel(BrokerState::Ready);
        let (events_tx, _) = broadcast::channel(64);
        let (control_tx, control_rx) = mpsc::channel(4);
        let task = tokio::spawn(serve_task(self, listener, state_tx, events_tx.clone(), control_rx));
        Ok(BrokerHandle { endpoint, state: state_rx, events: events_tx, control: control_tx, task: Some(task) })
    }
}

enum Command {
    Terminate { mode: TerminateMode, done: oneshot::Sender<()> },
}

/// A serving broker.
#[derive(Debug)]
pub struct BrokerHandle {
    endpoint: BrokerEndpoint,
    state: watch::Receiver<BrokerState>,
    events: broadcast::Sender<BrokerEvent>,
    control: mpsc::Sender<Command>,
    task: Option<JoinHandle<()>>,
}

impl BrokerHandle {
    pub fn endpoint(&self) -> &BrokerEndpoint {
        &self.endpoint
    }

    /// The URL LLDB connects to: `unix-connect:///…/gdb.sock` or `connect://127.0.0.1:<port>`.
    pub fn lldb_url(&self) -> String {
        lldb_url(&self.endpoint)
    }

    pub fn state(&self) -> BrokerState {
        *self.state.borrow()
    }

    /// A receiver of state changes.
    pub fn watch(&self) -> watch::Receiver<BrokerState> {
        self.state.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BrokerEvent> {
        self.events.subscribe()
    }

    /// Waits until `pred` holds or the broker is done; returns the state then.
    pub async fn wait_for(&self, pred: impl Fn(BrokerState) -> bool) -> BrokerState {
        let mut rx = self.state.clone();
        loop {
            let s = *rx.borrow_and_update();
            if pred(s) || s.is_final() {
                return s;
            }
            if rx.changed().await.is_err() {
                return *rx.borrow();
            }
        }
    }

    /// Kills or detaches the process, whether or not a debugger is connected, and waits until done.
    pub async fn terminate(&self, mode: TerminateMode) -> Result<()> {
        if self.state().is_final() {
            return Ok(());
        }
        let (done, wait) = oneshot::channel();
        if self.control.send(Command::Terminate { mode, done }).await.is_err() {
            return Ok(());
        }
        let _ = wait.await;
        Ok(())
    }
}

impl Drop for BrokerHandle {
    fn drop(&mut self) {
        // The serving task keeps the session alive; it ends with the process. Nothing to abort here: dropping
        // the handle must not kill a detached session.
        let _ = self.task.take();
    }
}

/// The LLDB URL of an endpoint.
pub fn lldb_url(endpoint: &BrokerEndpoint) -> String {
    match endpoint {
        BrokerEndpoint::Unix { path } => format!("unix-connect://{}", path.display()),
        BrokerEndpoint::Tcp { addr } => format!("connect://{addr}"),
    }
}

enum Listener {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener, PathBuf),
    Tcp(tokio::net::TcpListener),
}

impl Listener {
    async fn bind(spec: &EndpointSpec) -> Result<(Self, BrokerEndpoint)> {
        match spec {
            #[cfg(unix)]
            EndpointSpec::Unix { path } => {
                use std::os::unix::fs::PermissionsExt;
                if let Some(dir) = path.parent() {
                    crate::fsutil::private_dir(dir).map_err(|e| errors::io("creating the broker directory", e))?;
                }
                let _ = std::fs::remove_file(path);
                let l = tokio::net::UnixListener::bind(path).map_err(|e| errors::io("binding the broker socket", e))?;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| errors::io("restricting the broker socket", e))?;
                Ok((Self::Unix(l, path.clone()), BrokerEndpoint::Unix { path: path.clone() }))
            }
            EndpointSpec::Tcp { bind } => {
                if !bind.ip().is_loopback() {
                    return Err(russet_core::Error::new(
                        codes::core::INVALID_ARGUMENT,
                        "the broker listens on loopback only",
                    ));
                }
                let l =
                    tokio::net::TcpListener::bind(bind).await.map_err(|e| errors::io("binding the broker port", e))?;
                let addr = l.local_addr().map_err(|e| errors::io("reading the broker port", e))?;
                Ok((Self::Tcp(l), BrokerEndpoint::Tcp { addr }))
            }
        }
    }

    async fn accept(&self) -> std::io::Result<BoxStream> {
        match self {
            #[cfg(unix)]
            Self::Unix(l, _) => Ok(Box::new(l.accept().await?.0)),
            Self::Tcp(l) => {
                let (s, _) = l.accept().await?;
                let _ = s.set_nodelay(true);
                Ok(Box::new(s))
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Self::Unix(_, path) = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The broker's relay hooks for Swift/ObjC sessions (RD §6.4).
#[derive(Debug, Default)]
struct RelayHooks {
    events: Option<broadcast::Sender<BrokerEvent>>,
    pid: Option<u32>,
    breakpoints: BTreeSet<Vec<u8>>,
    detached: bool,
    killed: bool,
    exited: Option<(Option<i32>, Option<i32>)>,
}

impl InterposeHooks for RelayHooks {
    fn on_downstream(&mut self, pkt: &[u8]) -> DownstreamAction {
        let is_launch = pkt.starts_with(b"vAttach")
            || pkt.starts_with(b"vRun")
            || (pkt.first() == Some(&b'A') && pkt.get(1).is_some_and(u8::is_ascii_digit));
        if is_launch {
            let msg = format!("russet broker already attached pid {}", self.pid.unwrap_or(0));
            return DownstreamAction::Reply(
                format!("E16;{}", russet_gdbremote::codec::hex_encode(msg.as_bytes())).into_bytes(),
            );
        }
        // The broker owns the lost-connection policy (RD §4.6); LLDB's own value would override it.
        if pkt.starts_with(b"QSetDetachOnError") {
            return DownstreamAction::Reply(b"OK".to_vec());
        }
        // Breakpoints and watchpoints (`Z0`…`Z4`): the broker removes what a departed debugger left behind.
        if is_point(pkt, b'Z') {
            self.breakpoints.insert(pkt[1..].to_vec());
        } else if is_point(pkt, b'z') {
            self.breakpoints.remove(&pkt[1..]);
        } else if russet_gdbremote::interpose::is_resume(pkt)
            && let Some(tx) = &self.events
        {
            let _ = tx.send(BrokerEvent::Resumed);
        }
        DownstreamAction::Forward
    }

    fn on_upstream_stop(&mut self, stop: &StopEvent) -> UpstreamStopAction {
        match stop {
            StopEvent::Exited { code } => self.exited = Some((Some(i32::from(*code)), None)),
            StopEvent::Terminated { signal } => self.exited = Some((None, Some(i32::from(*signal)))),
            StopEvent::Stopped(r) => {
                if let Some(tx) = &self.events {
                    let _ = tx.send(BrokerEvent::Stopped { reason: r.reason.clone() });
                }
            }
            StopEvent::Output(_) => {}
        }
        UpstreamStopAction::Forward
    }

    fn filter_upstream_reply(&mut self, request: &[u8], reply: &mut Vec<u8>) {
        if request.starts_with(b"qSupported") {
            let text = String::from_utf8_lossy(reply).into_owned();
            *reply = text
                .split(';')
                .filter(|f| !f.starts_with("SupportedCompressions="))
                .collect::<Vec<_>>()
                .join(";")
                .into_bytes();
        } else if (request == b"D" || request.starts_with(b"D;")) && reply.as_slice() == b"OK" {
            self.detached = true;
        } else if request == b"k" {
            self.killed = true;
            if let Ok(StopEvent::Terminated { signal }) = russet_gdbremote::parse_stop(reply) {
                self.exited = Some((None, Some(i32::from(signal))));
            } else if let Ok(StopEvent::Exited { code }) = russet_gdbremote::parse_stop(reply) {
                self.exited = Some((Some(i32::from(code)), None));
            }
        }
    }
}

async fn serve_task(
    broker: Broker,
    listener: Listener,
    state: watch::Sender<BrokerState>,
    events: broadcast::Sender<BrokerEvent>,
    mut control: mpsc::Receiver<Command>,
) {
    let Broker { mut client, opts, pid, .. } = broker;
    let set = |s: BrokerState| {
        let _ = state.send(s);
    };
    let emit = |e: BrokerEvent| {
        let _ = events.send(e);
    };
    let mut first = true;
    let deadline = opts.handoff_timeout.map(|t| tokio::time::Instant::now() + t);
    loop {
        // Wait for a debugger, a command, or the handoff deadline.
        let handoff = async {
            match deadline {
                Some(d) if first => tokio::time::sleep_until(d).await,
                _ => std::future::pending::<()>().await,
            }
        };
        let downstream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "broker accept failed");
                    // A persistent error (for example no free descriptors) must not spin.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            cmd = control.recv() => {
                let (mode, done) = match cmd {
                    Some(Command::Terminate { mode, done }) => (mode, Some(done)),
                    // Every handle is gone: keep the default policy.
                    None => (TerminateMode::Kill, None),
                };
                let s = finish(client, mode, false, &[]).await;
                announce(&emit, s);
                set(s);
                if let Some(d) = done { let _ = d.send(()); }
                return;
            }
            () = handoff => {
                tracing::warn!(timeout = ?opts.handoff_timeout, "no debugger connected to the broker in time");
                // Announced once the app is gone, so whoever ends the session on it never races the kill.
                let _ = finish(client, TerminateMode::Kill, false, &[]).await;
                emit(BrokerEvent::HandoffTimeout);
                set(BrokerState::HandoffTimedOut);
                return;
            }
        };
        first = false;
        set(BrokerState::Relaying);
        emit(BrokerEvent::DebuggerConnected);
        let mut hooks = RelayHooks { events: Some(events.clone()), pid, ..RelayHooks::default() };
        let mut command: Option<Command> = None;
        let parts = client.into_parts();
        let outcome = Interposer::new(parts, downstream)
            .run(&mut hooks, async {
                command = control.recv().await;
            })
            .await;
        let out = match outcome {
            Ok(out) => out,
            Err(e) => {
                tracing::warn!(error = %e, "broker relay failed");
                emit(BrokerEvent::Lost);
                set(BrokerState::Lost);
                return;
            }
        };
        let resume_pending = out.resume_pending;
        let points: Vec<Vec<u8>> = hooks.breakpoints.iter().cloned().collect();
        client = GdbClient::from_parts(out.upstream);
        drop(out.downstream);
        // The debugger's `k` kills even when debugserver closed before its reply arrived.
        let exited = hooks.exited.or(hooks.killed.then_some((None, Some(9))));
        if let Some((code, signal)) = exited {
            emit(BrokerEvent::ProcessExited { code, signal, requested: hooks.killed });
            set(BrokerState::Exited { code, signal });
            if let Some(Command::Terminate { done, .. }) = command {
                let _ = done.send(());
            }
            return;
        }
        if hooks.detached {
            emit(BrokerEvent::DebuggerDetached);
            set(BrokerState::Detached);
            if let Some(Command::Terminate { done, .. }) = command {
                let _ = done.send(());
            }
            return;
        }
        match out.end {
            RelayEnd::UpstreamClosed => {
                emit(BrokerEvent::Lost);
                set(BrokerState::Lost);
                return;
            }
            RelayEnd::Stopped => {
                let (mode, done) = match command {
                    Some(Command::Terminate { mode, done }) => (mode, Some(done)),
                    None => (TerminateMode::Kill, None),
                };
                let s = finish(client, mode, resume_pending, &points).await;
                announce(&emit, s);
                set(s);
                if let Some(d) = done {
                    let _ = d.send(());
                }
                return;
            }
            RelayEnd::DownstreamClosed => {
                emit(BrokerEvent::DebuggerLost(opts.on_debugger_lost));
                let mode = match opts.on_debugger_lost {
                    LostPolicy::Kill => TerminateMode::Kill,
                    LostPolicy::Detach => TerminateMode::Detach,
                    LostPolicy::Keep => {
                        if resume_pending {
                            client.assume_running();
                        }
                        if resume_pending && client.interrupt().await.is_err() {
                            emit(BrokerEvent::Lost);
                            set(BrokerState::Lost);
                            return;
                        }
                        remove_points(&mut client, &points).await;
                        set(BrokerState::Ready);
                        continue;
                    }
                };
                let s = finish(client, mode, resume_pending, &points).await;
                announce(&emit, s);
                set(s);
                return;
            }
        }
    }
}

/// `Z0,`…`Z4,` (or `z…`): a breakpoint or watchpoint packet.
fn is_point(pkt: &[u8], lead: u8) -> bool {
    pkt.len() > 2 && pkt[0] == lead && (b'0'..=b'4').contains(&pkt[1]) && pkt[2] == b','
}

/// Removes the breakpoints and watchpoints a departed debugger left in debugserver.
async fn remove_points(client: &mut GdbClient<BoxStream>, points: &[Vec<u8>]) {
    for p in points {
        let mut packet = vec![b'z'];
        packet.extend_from_slice(p);
        if let Err(e) = client.request(&packet).await {
            tracing::warn!(error = %e, "removing a debugger's breakpoint failed");
        }
    }
}

/// Reports the outcome of a kill or detach the broker performed.
fn announce(emit: &impl Fn(BrokerEvent), s: BrokerState) {
    match s {
        BrokerState::Exited { code, signal } => emit(BrokerEvent::ProcessExited { code, signal, requested: true }),
        BrokerState::Lost => emit(BrokerEvent::Lost),
        _ => {}
    }
}

/// Kills or detaches through the upstream connection. A stub that is running must stop first; before a detach the
/// debugger's breakpoints come out, or the app would trap on them later.
async fn finish(
    mut client: GdbClient<BoxStream>,
    mode: TerminateMode,
    running: bool,
    points: &[Vec<u8>],
) -> BrokerState {
    match mode {
        TerminateMode::Kill => {
            if running {
                client.assume_running();
            }
            let killed = client.kill().await;
            match killed {
                Ok(Some(StopEvent::Exited { code })) => {
                    BrokerState::Exited { code: Some(i32::from(code)), signal: None }
                }
                Ok(Some(StopEvent::Terminated { signal })) => {
                    BrokerState::Exited { code: None, signal: Some(i32::from(signal)) }
                }
                // No reply before the stub closed: with QSetDetachOnError:0 it killed the process anyway.
                Ok(_) => BrokerState::Exited { code: None, signal: Some(9) },
                Err(_) => BrokerState::Lost,
            }
        }
        TerminateMode::Detach => {
            if running {
                client.assume_running();
                if client.interrupt().await.is_err() {
                    return BrokerState::Lost;
                }
            }
            remove_points(&mut client, points).await;
            match client.detach().await {
                Ok(()) => BrokerState::Detached,
                Err(GdbError::Disconnected) | Err(_) => BrokerState::Lost,
            }
        }
    }
}

/// A plain forward from one local endpoint to the device's debugserver, for `attachMode: legacy-async`: LLDB
/// does the handshake and the `vAttach` itself (the M1 sequence, ARCHITECTURE §10.3). One client; the forward
/// ends when either side closes.
#[derive(Debug)]
pub struct PlainForward {
    endpoint: BrokerEndpoint,
    task: JoinHandle<()>,
}

impl PlainForward {
    pub fn endpoint(&self) -> &BrokerEndpoint {
        &self.endpoint
    }
}

impl Drop for PlainForward {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Serves a [`PlainForward`] on `spec`; gives up when no client connects within `handoff`.
pub async fn serve_plain_forward(
    upstream: BoxStream,
    spec: EndpointSpec,
    handoff: Option<Duration>,
) -> Result<PlainForward> {
    let (listener, endpoint) = Listener::bind(&spec).await?;
    let task = tokio::spawn(async move {
        let accepted = match handoff {
            Some(t) => match tokio::time::timeout(t, listener.accept()).await {
                Ok(r) => r,
                Err(_) => {
                    tracing::warn!("no debugger connected to the forward in time");
                    return;
                }
            },
            None => listener.accept().await,
        };
        let Ok(mut downstream) = accepted else { return };
        let mut upstream = upstream;
        let _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
        drop(listener);
    });
    Ok(PlainForward { endpoint, task })
}

/// Binds `spec` and releases it at once; returns the LLDB URL it would have (the doctor check).
pub async fn probe_bind(spec: &EndpointSpec) -> Result<String> {
    let (listener, endpoint) = Listener::bind(spec).await?;
    drop(listener);
    #[cfg(unix)]
    if let BrokerEndpoint::Unix { path } = &endpoint
        && let Some(dir) = path.parent()
    {
        let _ = std::fs::remove_dir(dir);
    }
    Ok(lldb_url(&endpoint))
}

/// A Unix socket path for a session's broker under `runtime_dir/sessions/<sid>/<name>`, or, when that is longer
/// than `sun_path` allows, `<tmp>/russet-<uid>/<hash>-<name>` (ARCHITECTURE §7.1's rule for long paths).
pub fn socket_path(runtime_dir: &Path, sid: &str, name: &str) -> PathBuf {
    const MAX: usize = 100; // sun_path is 108 bytes on Linux and 104 on macOS, NUL included.
    let preferred = runtime_dir.join("sessions").join(sid).join(name);
    if preferred.as_os_str().len() <= MAX {
        return preferred;
    }
    let uid = crate::fsutil::current_uid().unwrap_or(0);
    let hash = crate::fsutil::fnv1a(preferred.as_os_str().as_encoded_bytes());
    std::env::temp_dir().join(format!("russet-{uid}")).join(format!("{hash:016x}-{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_hooks_refuse_attaches_and_keep_qsetdetachonerror() {
        let mut h = RelayHooks { pid: Some(932), ..RelayHooks::default() };
        match h.on_downstream(b"vAttach;3a4") {
            DownstreamAction::Reply(r) => {
                assert!(r.starts_with(b"E16;"));
                let msg = russet_gdbremote::codec::hex_decode(&r[4..]).unwrap_or_default();
                assert_eq!(msg, b"russet broker already attached pid 932");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(h.on_downstream(b"vRun;2f61"), DownstreamAction::Reply(_)));
        assert!(matches!(h.on_downstream(b"A10,0,2f61"), DownstreamAction::Reply(_)));
        assert_eq!(h.on_downstream(b"QSetDetachOnError:1"), DownstreamAction::Reply(b"OK".to_vec()));
        assert_eq!(h.on_downstream(b"Z0,100004000,4"), DownstreamAction::Forward);
        assert_eq!(h.breakpoints.len(), 1);
        assert_eq!(h.on_downstream(b"z0,100004000,4"), DownstreamAction::Forward);
        assert!(h.breakpoints.is_empty());
        assert_eq!(h.on_downstream(b"qProcessInfo"), DownstreamAction::Forward);
    }

    #[test]
    fn relay_hooks_strip_compressions_and_track_detach_and_kill() {
        let mut h = RelayHooks::default();
        let mut reply = b"PacketSize=20000;SupportedCompressions=lzfse,zlib-deflate;qEcho+".to_vec();
        h.filter_upstream_reply(b"qSupported:xmlRegisters=aarch64", &mut reply);
        assert_eq!(reply, b"PacketSize=20000;qEcho+");
        let mut ok = b"OK".to_vec();
        h.filter_upstream_reply(b"D", &mut ok);
        assert!(h.detached);
        let mut x = b"X09".to_vec();
        h.filter_upstream_reply(b"k", &mut x);
        assert!(h.killed);
        assert_eq!(h.exited, Some((None, Some(9))));
    }

    #[test]
    fn long_socket_paths_fall_back_to_a_short_one() {
        let short = socket_path(Path::new("/run/user/1000/russet"), "ses_01J00000000000000000000000", "gdb.sock");
        assert_eq!(short, Path::new("/run/user/1000/russet/sessions/ses_01J00000000000000000000000/gdb.sock"));
        let deep = "/tmp/".to_owned() + &"x".repeat(120);
        let long = socket_path(Path::new(&deep), "ses_01J00000000000000000000000", "gdb.sock");
        assert!(long.as_os_str().len() <= 100, "{}", long.display());
        assert!(long.to_string_lossy().ends_with("-gdb.sock"));
    }

    #[test]
    fn urls() {
        let unix = BrokerEndpoint::Unix { path: PathBuf::from("/run/user/1000/russet/sessions/s/gdb.sock") };
        assert_eq!(lldb_url(&unix), "unix-connect:///run/user/1000/russet/sessions/s/gdb.sock");
        let tcp = BrokerEndpoint::Tcp { addr: "127.0.0.1:41000".parse().unwrap_or_else(|_| unreachable!()) };
        assert_eq!(lldb_url(&tcp), "connect://127.0.0.1:41000");
    }
}
