// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The per-session DAP hub (RD §9.3, ARCHITECTURE §10.2, RD-07). Owner: W11.
//!
//! One hub per debug session fronts a single daemon-owned `lldb-dap` backend. Clients are the IDE (role
//! [`ClientRole::Ide`], over the session's Unix socket / named pipe) plus in-process agent clients (role
//! [`ClientRole::Agent`], used by the `debug.*` RPCs). The hub:
//!
//! - rewrites `seq`/`request_seq` per client, so each client has its own sequence space;
//! - routes a backend response to the one client that asked, and broadcasts events to every client;
//! - lets only an `ide` client send `disconnect`/`terminate`;
//! - answers Russet custom requests (`russet/hotReload`, `russet/relaunch`, `russet/jitStats`,
//!   `russet/deviceLogs`) itself through [`HubHooks`], without forwarding them to `lldb-dap`;
//! - caches the last `setBreakpoints`/`setFunctionBreakpoints`/`setExceptionBreakpoints` per source so a
//!   relaunch can replay them to a fresh backend ([`DapHub::cached_breakpoints`]).
//!
//! The backend is any [`crate::BoxStream`]: production wires `lldb-dap`'s stdio; tests inject a duplex pipe and a
//! scripted fake adapter. The IDE listener and backend-process spawn are the device-gated integration (DC-07,
//! DC-14); the multiplexer, routing, gating, custom requests and breakpoint cache are fully host-tested here.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_core::Result;
use russet_core::error::{Error, codes};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::BoxStream;

/// A client's role on the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientRole {
    /// The IDE: it owns `setBreakpoints`, `disconnect` and `terminate`.
    Ide,
    /// An in-process agent (the `debug.*` RPCs): it may drive execution but never disconnect/terminate.
    Agent,
}

/// What the hub needs the session to do for a Russet custom request (RD §9.3). The production hub forwards to the
/// [`crate::SessionHandle`]; tests use a fake.
#[async_trait::async_trait]
pub trait HubHooks: Send + Sync {
    /// `russet/hotReload` / `russet/relaunch`: Swift Quick Relaunch; returns the new pid.
    async fn relaunch(&self) -> Result<u32> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "relaunch is not available for this session"))
    }
    /// `russet/jitStats` (Flutter, W18); `None` for a Swift session.
    fn jit_stats(&self) -> Option<Value> {
        None
    }
    /// `russet/deviceLogs`: recent merged output as DAP-shaped entries.
    fn device_logs(&self, _since_ms: Option<i64>) -> Vec<Value> {
        Vec::new()
    }
}

/// A no-op hooks implementation.
pub struct NoHooks;
impl HubHooks for NoHooks {}

/// A command the hub actor processes.
enum HubCmd {
    /// The headless handshake already established the one shared backend.
    Bootstrapped(BootstrapState),
    /// A request from an agent client; the reply carries the response `body` (or an error).
    AgentRequest { role: ClientRole, command: String, arguments: Value, reply: oneshot::Sender<Result<Value>> },
    /// A framed message from the IDE socket client.
    IdeMessage(Value),
    /// The IDE connected; `sink` receives framed bytes to write to the socket.
    IdeAttached(mpsc::Sender<Vec<u8>>),
    /// The IDE socket closed.
    IdeGone,
}

struct BootstrapState {
    initialize: Value,
    attach: Value,
    config: Value,
}

/// The hub handle (`DaemonCtx` holds one per debug session).
pub struct DapHub {
    cmd_tx: mpsc::Sender<HubCmd>,
    events: broadcast::Sender<Value>,
    breakpoints: Arc<Mutex<BreakpointCache>>,
}

impl std::fmt::Debug for DapHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DapHub").finish_non_exhaustive()
    }
}

/// The last breakpoint sets per source, for relaunch replay (RD §9.3).
#[derive(Debug, Default, Clone)]
pub struct BreakpointCache {
    /// `setBreakpoints` arguments keyed by source path.
    pub sources: BTreeMap<String, Value>,
    /// The last `setFunctionBreakpoints` arguments.
    pub functions: Option<Value>,
    /// The last `setExceptionBreakpoints` arguments.
    pub exceptions: Option<Value>,
}

impl DapHub {
    /// Starts the hub over `backend` (an `lldb-dap` stdio stream, or a test duplex), driven by `hooks`.
    pub fn start(backend: BoxStream, hooks: Arc<dyn HubHooks>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(256);
        let (events, _) = broadcast::channel(256);
        let breakpoints = Arc::new(Mutex::new(BreakpointCache::default()));
        let actor = HubActor {
            hooks,
            events: events.clone(),
            breakpoints: Arc::clone(&breakpoints),
            next_backend_seq: AtomicU64::new(1),
            pending: BTreeMap::new(),
            ide: None,
            ide_seq: 0,
            bootstrap: None,
        };
        tokio::spawn(actor.run(backend, cmd_rx, cmd_tx.clone()));
        Self { cmd_tx, events, breakpoints }
    }

    /// An in-process agent client (role [`ClientRole::Agent`]) for the `debug.*` RPCs.
    pub fn agent_client(&self) -> DapClient {
        DapClient { cmd_tx: self.cmd_tx.clone(), role: ClientRole::Agent, events: self.events.subscribe() }
    }

    /// The cached breakpoint sets, for a relaunch to replay.
    pub fn cached_breakpoints(&self) -> BreakpointCache {
        self.breakpoints.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Drives the backend to the attached, stopped-or-running state with a headless `initialize` + `attach`
    /// (RD §9.2), so agents can debug without an IDE. The IDE, if it also connects, shares the session. `config`
    /// is `debug.prepare`'s `dapConfig` (with `attachCommands`/`initCommands`). Fails if the backend rejects it.
    pub async fn bootstrap_attach(&self, config: Value) -> Result<()> {
        let client = self.agent_client();
        let initialize = client
            .request(
                "initialize",
                json!({
                    "adapterID": "russet", "clientID": "russet-headless", "pathFormat": "path",
                    "linesStartAt1": true, "columnsStartAt1": true,
                }),
            )
            .await?;
        let attach = client.request("attach", config.clone()).await?;
        self.cmd_tx
            .send(HubCmd::Bootstrapped(BootstrapState { initialize, attach, config }))
            .await
            .map_err(|_| Error::new(codes::debug::BROKER_LOST, "the DAP backend is gone"))?;
        Ok(())
    }

    /// Attaches an IDE client reachable at `endpoint` by listening on its Unix socket (one acceptor). Returns at
    /// once; the accept and relay run in the background. Windows named-pipe support lands with W15/W27.
    #[cfg(unix)]
    pub async fn serve_ide(&self, endpoint: &crate::DapEndpoint) -> Result<()> {
        let crate::DapEndpoint::Unix { path } = endpoint else {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "the DAP hub needs a Unix endpoint on this host"));
        };
        if let Some(dir) = path.parent() {
            crate::fsutil::private_dir(dir).map_err(|e| crate::errors::io("creating the DAP socket directory", e))?;
        }
        let _ = std::fs::remove_file(path);
        let listener =
            tokio::net::UnixListener::bind(path).map_err(|e| crate::errors::io("binding the DAP socket", e))?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| crate::errors::io("securing the DAP socket", e))?;
        }
        let cmd_tx = self.cmd_tx.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                serve_ide_conn(stream, cmd_tx.clone()).await;
            }
        });
        Ok(())
    }
}

/// Relays one IDE connection to and from the hub actor.
#[cfg(unix)]
async fn serve_ide_conn(stream: tokio::net::UnixStream, cmd_tx: mpsc::Sender<HubCmd>) {
    let (mut read, mut write) = stream.into_split();
    let (to_ide_tx, mut to_ide_rx) = mpsc::channel::<Vec<u8>>(256);
    if cmd_tx.send(HubCmd::IdeAttached(to_ide_tx)).await.is_err() {
        return;
    }
    let writer = tokio::spawn(async move {
        while let Some(bytes) = to_ide_rx.recv().await {
            if write.write_all(&bytes).await.is_err() {
                break;
            }
            let _ = write.flush().await;
        }
    });
    while let Ok(Some(msg)) = read_message(&mut read).await {
        if cmd_tx.send(HubCmd::IdeMessage(msg)).await.is_err() {
            break;
        }
    }
    let _ = cmd_tx.send(HubCmd::IdeGone).await;
    writer.abort();
}

/// What the hub is waiting for a backend response to.
enum Pending {
    /// An agent request: the body (or error) goes to this oneshot.
    Agent(oneshot::Sender<Result<Value>>),
    /// An IDE request: rewrite `request_seq` to this value and forward to the IDE.
    Ide { ide_seq: u64 },
}

struct HubActor {
    hooks: Arc<dyn HubHooks>,
    events: broadcast::Sender<Value>,
    breakpoints: Arc<Mutex<BreakpointCache>>,
    next_backend_seq: AtomicU64,
    pending: BTreeMap<u64, Pending>,
    ide: Option<mpsc::Sender<Vec<u8>>>,
    ide_seq: u64,
    bootstrap: Option<BootstrapState>,
}

impl HubActor {
    async fn run(mut self, backend: BoxStream, mut cmd_rx: mpsc::Receiver<HubCmd>, cmd_tx: mpsc::Sender<HubCmd>) {
        let (mut br, mut bw) = tokio::io::split(backend);
        // A task that frames backend messages into `backend_rx`.
        let (backend_tx, mut backend_rx) = mpsc::channel::<Value>(256);
        tokio::spawn(async move {
            while let Ok(Some(msg)) = read_message(&mut br).await {
                if backend_tx.send(msg).await.is_err() {
                    break;
                }
            }
        });
        drop(cmd_tx);
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(cmd) => self.on_client(cmd, &mut bw).await,
                    None => break,
                },
                msg = backend_rx.recv() => match msg {
                    Some(msg) => self.on_backend(msg).await,
                    None => break,   // the backend closed
                },
            }
        }
    }

    async fn on_client<W: AsyncWrite + Unpin>(&mut self, cmd: HubCmd, backend: &mut W) {
        match cmd {
            HubCmd::Bootstrapped(state) => self.bootstrap = Some(state),
            HubCmd::AgentRequest { role, command, arguments, reply } => {
                self.handle_request(role, command, arguments, Caller::Agent(reply), 0, backend).await;
            }
            HubCmd::IdeMessage(msg) => self.on_ide_message(msg, backend).await,
            HubCmd::IdeAttached(sink) => self.ide = Some(sink),
            HubCmd::IdeGone => self.ide = None,
        }
    }

    async fn on_ide_message<W: AsyncWrite + Unpin>(&mut self, msg: Value, backend: &mut W) {
        match msg.get("type").and_then(Value::as_str) {
            Some("request") => {
                let command = msg.get("command").and_then(Value::as_str).unwrap_or_default().to_owned();
                let arguments = msg.get("arguments").cloned().unwrap_or(Value::Null);
                let ide_seq = msg.get("seq").and_then(Value::as_u64).unwrap_or(0);
                self.handle_request(ClientRole::Ide, command, arguments, Caller::Ide, ide_seq, backend).await;
            }
            // Responses to reverse requests from the backend: forward to the backend as-is.
            Some("response") => {
                let _ = write_message(backend, &msg).await;
            }
            _ => {}
        }
    }

    /// Core request handling: custom requests and disconnect gating are resolved here; everything else is
    /// assigned a backend seq and forwarded, with the caller recorded for the response.
    async fn handle_request<W: AsyncWrite + Unpin>(
        &mut self,
        role: ClientRole,
        command: String,
        arguments: Value,
        caller: Caller,
        ide_seq: u64,
        backend: &mut W,
    ) {
        // An IDE joining an agent-prepared session must share its established
        // backend. LLDB accepts one initialize/attach handshake per process.
        if role == ClientRole::Ide
            && let Some(state) = &self.bootstrap
        {
            if command == "initialize" {
                caller.respond(&command, ide_seq, Ok(state.initialize.clone()), &self.ide).await;
                return;
            }
            if command == "attach" {
                let matches = arguments.get("attachCommands") == state.config.get("attachCommands")
                    && arguments.get("initCommands") == state.config.get("initCommands");
                if !matches {
                    let error = Error::new(
                        codes::core::INVALID_ARGUMENT,
                        "IDE attach configuration differs from the prepared session",
                    );
                    caller.respond(&command, ide_seq, Err(error), &self.ide).await;
                    return;
                }
                caller.respond(&command, ide_seq, Ok(state.attach.clone()), &self.ide).await;
                self.send_to_ide(json!({"type":"event", "event":"initialized"})).await;
                return;
            }
        }
        // Only the IDE may end the session.
        if matches!(command.as_str(), "disconnect" | "terminate") && role != ClientRole::Ide {
            let e = Error::new(codes::core::INVALID_ARGUMENT, "only the IDE may disconnect or terminate the session")
                .with_hint("Agents detach with `run.stop {mode: detach}`.");
            caller.respond(&command, ide_seq, Err(e), &self.ide).await;
            return;
        }
        // Russet custom requests are answered by the hub.
        if let Some(stripped) = command.strip_prefix("russet/") {
            let body = self.custom_request(stripped, &arguments).await;
            caller.respond(&command, ide_seq, body, &self.ide).await;
            return;
        }
        // Cache breakpoint sets for relaunch replay.
        self.cache_breakpoints(&command, &arguments);
        // Forward to the backend with a hub-assigned seq.
        let backend_seq = self.next_backend_seq.fetch_add(1, Ordering::Relaxed);
        let forwarded = json!({"seq": backend_seq, "type": "request", "command": command, "arguments": arguments});
        match caller {
            Caller::Agent(reply) => {
                self.pending.insert(backend_seq, Pending::Agent(reply));
            }
            Caller::Ide => {
                self.pending.insert(backend_seq, Pending::Ide { ide_seq });
            }
        }
        if write_message(backend, &forwarded).await.is_err()
            && let Some(Pending::Agent(reply)) = self.pending.remove(&backend_seq)
        {
            let _ = reply.send(Err(Error::new(codes::debug::BROKER_LOST, "the DAP backend is gone")));
        }
    }

    async fn custom_request(&self, name: &str, args: &Value) -> Result<Value> {
        match name {
            "hotReload" | "relaunch" => {
                let pid = self.hooks.relaunch().await?;
                Ok(json!({ "pid": pid }))
            }
            "jitStats" => Ok(self.hooks.jit_stats().unwrap_or(Value::Null)),
            "deviceLogs" => {
                let since = args.get("sinceMs").and_then(Value::as_i64);
                Ok(json!({ "entries": self.hooks.device_logs(since) }))
            }
            other => Err(Error::new(codes::core::INVALID_ARGUMENT, format!("unknown custom request russet/{other}"))),
        }
    }

    fn cache_breakpoints(&self, command: &str, arguments: &Value) {
        let mut cache = self.breakpoints.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match command {
            "setBreakpoints" => {
                if let Some(path) = arguments.pointer("/source/path").and_then(Value::as_str) {
                    cache.sources.insert(path.to_owned(), arguments.clone());
                }
            }
            "setFunctionBreakpoints" => cache.functions = Some(arguments.clone()),
            "setExceptionBreakpoints" => cache.exceptions = Some(arguments.clone()),
            _ => {}
        }
    }

    async fn on_backend(&mut self, msg: Value) {
        match msg.get("type").and_then(Value::as_str) {
            Some("response") => {
                let backend_seq = msg.get("request_seq").and_then(Value::as_u64).unwrap_or(0);
                match self.pending.remove(&backend_seq) {
                    Some(Pending::Agent(reply)) => {
                        let _ = reply.send(response_to_result(&msg));
                    }
                    Some(Pending::Ide { ide_seq }) => {
                        let mut rewritten = msg;
                        rewritten["request_seq"] = json!(ide_seq);
                        self.send_to_ide(rewritten).await;
                    }
                    None => {} // a stale or unknown response
                }
            }
            Some("event") => {
                // Broadcast to agents, and forward to the IDE.
                let _ = self.events.send(msg.clone());
                self.send_to_ide(msg).await;
            }
            // A reverse request from the backend (e.g. startDebugging): forward to the IDE when present.
            Some("request") => self.send_to_ide(msg).await,
            _ => {}
        }
    }

    /// Forwards a message to the IDE with a fresh IDE-side seq.
    async fn send_to_ide(&mut self, mut msg: Value) {
        let Some(sink) = &self.ide else { return };
        self.ide_seq += 1;
        msg["seq"] = json!(self.ide_seq);
        if let Ok(bytes) = encode(&msg)
            && sink.send(bytes).await.is_err()
        {
            self.ide = None;
        }
    }
}

/// Who asked for a request, for routing the response.
enum Caller {
    Agent(oneshot::Sender<Result<Value>>),
    Ide,
}

impl Caller {
    async fn respond(self, command: &str, ide_seq: u64, body: Result<Value>, ide: &Option<mpsc::Sender<Vec<u8>>>) {
        match self {
            Caller::Agent(reply) => {
                let _ = reply.send(body);
            }
            Caller::Ide => {
                let msg = match body {
                    Ok(b) => {
                        json!({"type": "response", "request_seq": ide_seq, "success": true, "command": command, "body": b})
                    }
                    Err(e) => error_response(command, ide_seq, &e),
                };
                if let (Some(sink), Ok(bytes)) = (ide, encode(&msg)) {
                    let _ = sink.send(bytes).await;
                }
            }
        }
    }
}

/// A DAP client (agent or, internally, the IDE relay).
pub struct DapClient {
    cmd_tx: mpsc::Sender<HubCmd>,
    role: ClientRole,
    events: broadcast::Receiver<Value>,
}

impl DapClient {
    /// Sends a DAP request and returns its response `body` (or maps `success: false` to an error).
    pub async fn request(&self, command: &str, arguments: Value) -> Result<Value> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(HubCmd::AgentRequest { role: self.role, command: command.to_owned(), arguments, reply })
            .await
            .map_err(|_| Error::new(codes::debug::BROKER_LOST, "the DAP hub is gone"))?;
        rx.await.map_err(|_| Error::new(codes::debug::BROKER_LOST, "the DAP hub dropped the request"))?
    }

    /// Waits up to `timeout` for a `stopped` event and returns its `body`, or `None` on timeout.
    pub async fn next_stopped(&mut self, timeout: Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Ok(ev)) if ev.get("event").and_then(Value::as_str) == Some("stopped") => {
                    return Some(ev.get("body").cloned().unwrap_or(Value::Null));
                }
                Ok(Ok(_)) => {} // another event
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                _ => return None,
            }
        }
    }
}

/// Agent translation: the `debug.*` RPCs over a [`DapClient`]. These map protocol requests to DAP requests and
/// shape the DAP responses into the protocol types (ARCHITECTURE §8.5). They are pure given a client, so they are
/// host-tested against the fake backend.
pub mod agent {
    use russet_protocol::debug::{
        ControlAction, DebugControlResult, DebugEvaluateResult, DebugInspectResult, StackFrame, StopInfo, ThreadInfo,
        Variable,
    };
    use serde_json::{Value, json};

    use super::DapClient;
    use crate::Result;

    /// `debug.inspect {what: threads}`.
    pub async fn threads(client: &DapClient) -> Result<DebugInspectResult> {
        let body = client.request("threads", Value::Null).await?;
        let threads = body
            .get("threads")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().map(thread_info).collect())
            .unwrap_or_default();
        Ok(DebugInspectResult { threads: Some(threads), ..Default::default() })
    }

    /// `debug.inspect {what: stack}`.
    pub async fn stack(client: &DapClient, thread_id: Option<u64>, levels: Option<u32>) -> Result<DebugInspectResult> {
        let frames = stack_frames(client, thread_id, levels).await?;
        Ok(DebugInspectResult { stack_frames: Some(frames), ..Default::default() })
    }

    /// `debug.inspect {what: variables}`: the locals of a frame (or a DAP `variablesReference` via `scope`).
    pub async fn variables(
        client: &DapClient,
        frame_id: Option<u64>,
        scope: Option<&str>,
    ) -> Result<DebugInspectResult> {
        let reference = match scope.and_then(|s| s.parse::<u64>().ok()) {
            Some(r) => r,
            None => scope_reference(client, frame_id, scope).await?,
        };
        let vars = if reference == 0 {
            Vec::new()
        } else {
            let body = client.request("variables", json!({ "variablesReference": reference })).await?;
            body.get("variables")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().map(variable).collect())
                .unwrap_or_default()
        };
        Ok(DebugInspectResult { variables: Some(vars), ..Default::default() })
    }

    /// `debug.evaluate`.
    pub async fn evaluate(
        client: &DapClient,
        expression: &str,
        frame_id: Option<u64>,
        context: &str,
    ) -> Result<DebugEvaluateResult> {
        let mut args = json!({ "expression": expression, "context": context });
        if let Some(f) = frame_id {
            args["frameId"] = json!(f);
        }
        let body = client.request("evaluate", args).await?;
        Ok(DebugEvaluateResult {
            result: body.get("result").and_then(Value::as_str).unwrap_or_default().to_owned(),
            ty: body.get("type").and_then(Value::as_str).map(str::to_owned),
            variables_reference: body.get("variablesReference").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// `debug.control`: map the action to a DAP execution request, then (optionally) wait for the next stop.
    pub async fn control(
        client: &mut DapClient,
        action: ControlAction,
        thread_id: Option<u64>,
        wait_for_stop_ms: Option<u64>,
    ) -> Result<DebugControlResult> {
        let (command, needs_thread) = match action {
            ControlAction::Continue => ("continue", false),
            ControlAction::Pause => ("pause", true),
            ControlAction::StepOver => ("next", true),
            ControlAction::StepInto => ("stepIn", true),
            ControlAction::StepOut => ("stepOut", true),
        };
        let mut args = json!({});
        if let Some(t) = thread_id {
            args["threadId"] = json!(t);
        } else if needs_thread {
            args["threadId"] = json!(1);
        }
        client.request(command, args).await?;
        match wait_for_stop_ms.filter(|ms| *ms > 0) {
            Some(ms) => match client.next_stopped(std::time::Duration::from_millis(ms)).await {
                Some(body) => {
                    let tid = body.get("threadId").and_then(Value::as_u64).or(thread_id);
                    let frames = stack_frames(client, tid, Some(64)).await.unwrap_or_default();
                    Ok(DebugControlResult {
                        state: "stopped".to_owned(),
                        stop: Some(StopInfo {
                            reason: body.get("reason").and_then(Value::as_str).unwrap_or("stopped").to_owned(),
                            thread_id: tid,
                            description: body.get("description").and_then(Value::as_str).map(str::to_owned),
                            frames,
                        }),
                    })
                }
                None => Ok(DebugControlResult { state: "running".to_owned(), stop: None }),
            },
            None => Ok(DebugControlResult {
                state: if matches!(action, ControlAction::Pause) { "stopped" } else { "running" }.to_owned(),
                stop: None,
            }),
        }
    }

    async fn stack_frames(client: &DapClient, thread_id: Option<u64>, levels: Option<u32>) -> Result<Vec<StackFrame>> {
        let body = client
            .request("stackTrace", json!({ "threadId": thread_id.unwrap_or(1), "levels": levels.unwrap_or(0) }))
            .await?;
        Ok(body
            .get("stackFrames")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().map(stack_frame).collect())
            .unwrap_or_default())
    }

    /// Resolves the `variablesReference` for a frame's scope (the first matching scope, or the first scope).
    async fn scope_reference(client: &DapClient, frame_id: Option<u64>, scope: Option<&str>) -> Result<u64> {
        let Some(frame) = frame_id else { return Ok(0) };
        let body = client.request("scopes", json!({ "frameId": frame })).await?;
        let scopes = body.get("scopes").and_then(Value::as_array).cloned().unwrap_or_default();
        let pick = scope
            .and_then(|want| {
                scopes
                    .iter()
                    .find(|s| s.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(want)))
            })
            .or_else(|| scopes.first());
        Ok(pick.and_then(|s| s.get("variablesReference").and_then(Value::as_u64)).unwrap_or(0))
    }

    fn thread_info(v: &Value) -> ThreadInfo {
        ThreadInfo {
            id: v.get("id").and_then(Value::as_u64).unwrap_or(0),
            name: v.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
        }
    }

    fn stack_frame(v: &Value) -> StackFrame {
        StackFrame {
            id: v.get("id").and_then(Value::as_u64).unwrap_or(0),
            name: v.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
            source: v.pointer("/source/path").and_then(Value::as_str).map(str::to_owned),
            line: v.get("line").and_then(Value::as_u64).and_then(|l| u32::try_from(l).ok()),
            column: v.get("column").and_then(Value::as_u64).and_then(|c| u32::try_from(c).ok()),
            instruction_pointer: v.get("instructionPointerReference").and_then(Value::as_str).map(str::to_owned),
        }
    }

    fn variable(v: &Value) -> Variable {
        Variable {
            name: v.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
            value: v.get("value").and_then(Value::as_str).unwrap_or_default().to_owned(),
            ty: v.get("type").and_then(Value::as_str).map(str::to_owned),
            variables_reference: v.get("variablesReference").and_then(Value::as_u64).unwrap_or(0),
        }
    }
}

/// A DAP response's result: the `body` when `success`, else a mapped error (RD §22).
fn response_to_result(msg: &Value) -> Result<Value> {
    if msg.get("success").and_then(Value::as_bool).unwrap_or(false) {
        Ok(msg.get("body").cloned().unwrap_or(Value::Null))
    } else {
        let message = msg
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| msg.pointer("/body/error/format").and_then(Value::as_str))
            .unwrap_or("the debugger rejected the request")
            .to_owned();
        Err(Error::new(codes::debug::ATTACH_DENIED, message))
    }
}

fn error_response(command: &str, request_seq: u64, e: &Error) -> Value {
    json!({
        "type": "response", "request_seq": request_seq, "success": false, "command": command,
        "message": e.message(),
        "body": {"error": {"id": e.code().number(), "format": e.message()}}
    })
}

/// Spawns `lldb-dap` as the hub backend, returning its stdio as one duplex stream. The resolver env (the
/// libpython contract, ARCHITECTURE §16.2) is applied. Device/CI note: this is the production backend; tests
/// inject a duplex instead.
pub fn spawn_lldb_dap(
    program: &std::path::Path,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
) -> Result<BoxStream> {
    use std::process::Stdio;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).envs(env).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| crate::errors::io(&format!("spawning {}", program.display()), e))?;
    let stdin = child.stdin.take().ok_or_else(|| Error::new(codes::core::INTERNAL, "lldb-dap has no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| Error::new(codes::core::INTERNAL, "lldb-dap has no stdout"))?;
    Ok(Box::new(ChildStdio { _child: child, stdin, stdout }))
}

/// A child process's stdio as one `AsyncRead + AsyncWrite` stream (read = stdout, write = stdin). Dropping it
/// kills the child (`kill_on_drop`).
struct ChildStdio {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

impl AsyncRead for ChildStdio {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for ChildStdio {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.stdin).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdin).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

// ---- DAP framing (Content-Length) -------------------------------------------------------------------------

/// Encodes a DAP message with its `Content-Length` header.
pub fn encode(msg: &Value) -> std::io::Result<Vec<u8>> {
    let body = serde_json::to_vec(msg)?;
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    Ok(out)
}

async fn write_message<W: AsyncWrite + Unpin>(w: &mut W, msg: &Value) -> std::io::Result<()> {
    let bytes = encode(msg)?;
    w.write_all(&bytes).await?;
    w.flush().await
}

/// Largest DAP message body accepted (a hostile or confused peer cannot force a huge allocation).
const MAX_MESSAGE: usize = 32 << 20;

/// Reads one DAP message, or `None` at a clean end of stream.
pub async fn read_message<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        if !read_line(r, &mut line).await? {
            return Ok(None); // EOF before any header
        }
        let text = String::from_utf8_lossy(&line);
        let trimmed = text.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some((k, v)) = trimmed.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            content_length = v.trim().parse().ok();
        }
    }
    let len = content_length.ok_or_else(|| std::io::Error::other("DAP message without Content-Length"))?;
    if len > MAX_MESSAGE {
        return Err(std::io::Error::other("DAP message too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    let msg = serde_json::from_slice(&body).map_err(std::io::Error::other)?;
    Ok(Some(msg))
}

/// Reads one `\n`-terminated line into `buf`; returns `false` at an immediate EOF.
async fn read_line<R: AsyncRead + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> std::io::Result<bool> {
    let mut byte = [0u8; 1];
    let mut any = false;
    loop {
        match r.read(&mut byte).await? {
            0 => return Ok(any),
            _ => {
                any = true;
                buf.push(byte[0]);
                if byte[0] == b'\n' {
                    return Ok(true);
                }
                if buf.len() > 64 << 10 {
                    return Err(std::io::Error::other("DAP header line too long"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A scripted fake `lldb-dap`: it answers requests and emits a `stopped` event after `continue`.
    async fn fake_backend(mut stream: BoxStream) {
        loop {
            let msg = match read_message(&mut stream).await {
                Ok(Some(m)) => m,
                _ => return,
            };
            let seq = msg.get("seq").and_then(Value::as_u64).unwrap_or(0);
            let command = msg.get("command").and_then(Value::as_str).unwrap_or_default().to_owned();
            let body = match command.as_str() {
                "threads" => json!({"threads": [{"id": 1, "name": "main"}]}),
                "stackTrace" => {
                    json!({"stackFrames": [{"id": 1000, "name": "ContentView.body", "line": 12}], "totalFrames": 1})
                }
                "evaluate" => json!({"result": "42", "type": "Int", "variablesReference": 0}),
                "scopes" => json!({"scopes": [{"name": "Locals", "variablesReference": 77}]}),
                "variables" => {
                    json!({"variables": [{"name": "probeValue", "value": "42", "type": "Int", "variablesReference": 0}]})
                }
                "continue" | "next" | "stepIn" | "stepOut" | "pause" => json!({"allThreadsContinued": true}),
                _ => json!({}),
            };
            let resp =
                json!({"type": "response", "request_seq": seq, "success": true, "command": command, "body": body});
            write_message(&mut stream, &resp).await.unwrap();
            if matches!(command.as_str(), "continue" | "next" | "stepIn" | "stepOut") {
                let ev = json!({"type": "event", "event": "stopped", "body": {"reason": "breakpoint", "threadId": 1}});
                write_message(&mut stream, &ev).await.unwrap();
            }
        }
    }

    fn start_hub() -> DapHub {
        let (a, b) = tokio::io::duplex(64 * 1024);
        tokio::spawn(fake_backend(Box::new(b)));
        DapHub::start(Box::new(a), Arc::new(NoHooks))
    }

    #[tokio::test]
    async fn agent_requests_get_their_responses_and_events_broadcast() {
        let hub = start_hub();
        let mut agent = hub.agent_client();
        let threads = agent.request("threads", Value::Null).await.unwrap();
        assert_eq!(threads["threads"][0]["name"], "main");
        let eval = agent.request("evaluate", json!({"expression": "counter"})).await.unwrap();
        assert_eq!(eval["result"], "42");
        // continue → a stopped event reaches the agent.
        agent.request("continue", json!({"threadId": 1})).await.unwrap();
        let stop = agent.next_stopped(Duration::from_secs(2)).await.unwrap();
        assert_eq!(stop["reason"], "breakpoint");
    }

    #[tokio::test]
    async fn two_agents_each_see_their_own_responses_and_both_see_events() {
        let hub = start_hub();
        let a = hub.agent_client();
        let mut b = hub.agent_client();
        // Interleave requests from two clients; each gets its own response.
        let (ra, rb) =
            tokio::join!(a.request("stackTrace", json!({"threadId": 1})), b.request("threads", Value::Null),);
        assert_eq!(ra.unwrap()["stackFrames"][0]["name"], "ContentView.body");
        assert_eq!(rb.unwrap()["threads"][0]["id"], 1);
        a.request("continue", json!({"threadId": 1})).await.unwrap();
        // Both clients see the broadcast stopped event.
        assert!(b.next_stopped(Duration::from_secs(2)).await.is_some());
    }

    #[tokio::test]
    async fn an_agent_cannot_disconnect_or_terminate() {
        let hub = start_hub();
        let agent = hub.agent_client();
        let e = agent.request("disconnect", Value::Null).await.unwrap_err();
        assert_eq!(e.code(), codes::core::INVALID_ARGUMENT);
        assert!(agent.request("terminate", Value::Null).await.is_err());
    }

    #[tokio::test]
    async fn custom_requests_are_answered_by_the_hub_and_breakpoints_cached() {
        struct Hooks;
        #[async_trait::async_trait]
        impl HubHooks for Hooks {
            async fn relaunch(&self) -> Result<u32> {
                Ok(4321)
            }
            fn device_logs(&self, _since: Option<i64>) -> Vec<Value> {
                vec![json!({"message": "PROBE seq=1"})]
            }
        }
        let (a, b) = tokio::io::duplex(64 * 1024);
        tokio::spawn(fake_backend(Box::new(b)));
        let hub = DapHub::start(Box::new(a), Arc::new(Hooks));
        let agent = hub.agent_client();
        let r = agent.request("russet/relaunch", Value::Null).await.unwrap();
        assert_eq!(r["pid"], 4321);
        let logs = agent.request("russet/deviceLogs", json!({"sinceMs": 0})).await.unwrap();
        assert_eq!(logs["entries"][0]["message"], "PROBE seq=1");
        // Breakpoints are cached for relaunch replay.
        agent
            .request(
                "setBreakpoints",
                json!({"source": {"path": "/p/ContentView.swift"}, "breakpoints": [{"line": 12}]}),
            )
            .await
            .unwrap();
        let cache = hub.cached_breakpoints();
        assert!(cache.sources.contains_key("/p/ContentView.swift"));
        assert_eq!(cache.sources["/p/ContentView.swift"]["breakpoints"][0]["line"], 12);
    }

    #[tokio::test]
    async fn agent_translation_shapes_the_protocol_types() {
        use russet_protocol::debug::ControlAction;
        let hub = start_hub();
        let mut client = hub.agent_client();
        let threads = super::agent::threads(&client).await.unwrap();
        assert_eq!(threads.threads.as_ref().unwrap()[0].name, "main");
        let stack = super::agent::stack(&client, Some(1), Some(64)).await.unwrap();
        assert_eq!(stack.stack_frames.as_ref().unwrap()[0].name, "ContentView.body");
        assert_eq!(stack.stack_frames.as_ref().unwrap()[0].line, Some(12));
        // variables via a frame's scope (scopes → variablesReference → variables).
        let vars = super::agent::variables(&client, Some(1000), Some("locals")).await.unwrap();
        assert_eq!(vars.variables.as_ref().unwrap()[0].name, "probeValue");
        let eval = super::agent::evaluate(&client, "counter", Some(1000), "repl").await.unwrap();
        assert_eq!((eval.result.as_str(), eval.ty.as_deref()), ("42", Some("Int")));
        // control continue + wait returns the resulting stop with frames.
        let r = super::agent::control(&mut client, ControlAction::Continue, Some(1), Some(1000)).await.unwrap();
        assert_eq!(r.state, "stopped");
        let stop = r.stop.unwrap();
        assert_eq!(stop.reason, "breakpoint");
        assert!(!stop.frames.is_empty());
        // control without a wait returns running at once.
        let r = super::agent::control(&mut client, ControlAction::StepOver, Some(1), None).await.unwrap();
        assert_eq!(r.state, "running");
    }

    #[test]
    fn framing_round_trips() {
        let msg = json!({"type": "request", "seq": 7, "command": "threads"});
        let bytes = encode(&msg).unwrap();
        assert!(bytes.starts_with(b"Content-Length: "));
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut cursor = std::io::Cursor::new(bytes);
            let back = read_message(&mut cursor).await.unwrap().unwrap();
            assert_eq!(back, msg);
            // A clean EOF yields None.
            let mut empty = std::io::Cursor::new(Vec::new());
            assert!(read_message(&mut empty).await.unwrap().is_none());
        });
    }
}
