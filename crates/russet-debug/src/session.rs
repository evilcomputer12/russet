// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Sessions: state, debugger and lease bookkeeping, the output view, persistence and termination.
//!
//! A session's wire view is ARCHITECTURE §9.5's `SessionSummary` [D 84]: the state is one of the `SessionState`
//! values, and whether a debugger is attached is `debugger` (`none`, `attached`, `detached`), never a state.
//! Files (ARCHITECTURE §14.8): `state/sessions/<sid>/{session.json,output.log}` and `state/sessions.json`, the
//! list of live sessions for crash cleanup; the last [`KEEP_SESSIONS`] session directories are kept.
//! `session.json` has RD §21's fields (device, app, launch, broker, LLDB), filled in as the session learns them,
//! and its `outcome` says how the session ended, with the error when Russet knows one (for example
//! `debug.handoffTimeout`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use russet_core::Result;
use russet_core::ids::SessionId;
use russet_device::Udid;
use russet_protocol::common::{
    AttachMode, DebuggerState, LogLevel, SessionExit, SessionKind, SessionLease, SessionState as WireState,
    SessionSummary,
};
use russet_protocol::run::SessionError;
use serde_json::json;
use tokio::sync::{broadcast, watch};

use crate::broker::{BrokerHandle, BrokerState};
use crate::logs::{self, LogView};
use crate::{DeviceServices, HotReloadResult, SessionEvent, SessionState, TerminateMode};

/// How many ended session directories stay under `state/sessions/` (RD §12).
pub const KEEP_SESSIONS: usize = 20;
/// SIGKILL.
const SIGKILL: i32 = 9;

/// A launched process, reachable through the device plane.
pub(crate) struct ProcessRef {
    pub dev: Arc<dyn DeviceServices>,
    pub udid: Udid,
    pub pid: u32,
}

impl ProcessRef {
    pub(crate) async fn kill(&self) -> Result<()> {
        self.dev.signal(&self.udid, self.pid, SIGKILL).await
    }
}

/// Kills what a control holds, ignoring errors (the session is already over).
pub(crate) async fn stop_control(control: Control) {
    match control {
        Control::Broker { handle, process } => {
            let _ = handle.terminate(TerminateMode::Kill).await;
            if handle.state() == BrokerState::Detached {
                let _ = process.kill().await;
            }
        }
        Control::Process(process) => {
            let _ = process.kill().await;
        }
        Control::None => {}
    }
}

/// What [`SessionHandle::terminate`] acts on.
pub(crate) enum Control {
    /// Nothing to stop (not launched yet, or already ended).
    None,
    /// The broker owns the debugserver connection; `process` is for after a detach.
    Broker { handle: BrokerHandle, process: ProcessRef },
    /// A process without a debugger: stopped with a signal.
    Process(ProcessRef),
}

pub(crate) struct Ended {
    state: SessionState,
    at: String,
    exit: Option<SessionExit>,
    error: Option<SessionError>,
}

/// What `session.json` records about the device (RD §21); the device plane fills it when it can
/// ([`DeviceServices::describe`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceFacts {
    /// For example `iPhone13,2`.
    pub product_type: Option<String>,
    /// For example `27.0`.
    pub product_version: Option<String>,
    /// For example `24A437`.
    pub build_version: Option<String>,
    /// `usb` or `wifi`.
    pub transport: Option<String>,
}

/// The LLDB a debug session prepared (RD §21 `lldb`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LldbFacts {
    /// `swift` or `llvm`.
    pub flavor: String,
    pub version: String,
    /// The libpython it runs with, for example `cpython-3.12.15-linux-x86_64-gnu` (from `PYTHONHOME`).
    pub python: Option<String>,
}

/// RD §21's `session.json` fields beyond the summary, filled in as the session learns them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFacts {
    pub device: DeviceFacts,
    /// The app bundle on the device.
    pub app_device_path: Option<String>,
    /// `CFBundleExecutable`.
    pub app_executable: Option<String>,
    /// The local executable's `LC_UUID`.
    pub app_uuid: Option<String>,
    /// The launch strategy: `auto`, `coredevice` or `dvt`.
    pub launch: Option<String>,
    /// The broker URL LLDB connects to.
    pub lldb_url: Option<String>,
    pub lldb: Option<LldbFacts>,
}

pub(crate) struct Session {
    pub(crate) id: SessionId,
    pub(crate) kind: SessionKind,
    pub(crate) bundle_id: String,
    pub(crate) udid: Mutex<String>,
    pub(crate) project: Option<String>,
    pub(crate) lease: SessionLease,
    pub(crate) attach_mode: Option<AttachMode>,
    pub(crate) started_at: String,
    pub(crate) pid: AtomicU32,
    /// Bumped by every (re)launch so a stale exit watcher of a replaced process cannot end the session.
    generation: AtomicU32,
    state: watch::Sender<SessionState>,
    debugger: watch::Sender<DebuggerState>,
    ended: Mutex<Option<Ended>>,
    kill_requested: AtomicBool,
    events: broadcast::Sender<SessionEvent>,
    pub(crate) logs: LogView,
    control: tokio::sync::Mutex<Control>,
    dir: Option<PathBuf>,
    registry: Option<PathBuf>,
    /// Kept alive while the session runs (the launch channel in legacy-async mode, relay tasks).
    keep: Mutex<Vec<Box<dyn std::any::Any + Send>>>,
    /// The device's exit notice and the end of stdout, held back while the broker decides how the process ended.
    exit_watch: Mutex<Option<ExitSignals>>,
    /// The per-session DAP hub (set when a debug session has a DAP front end, ARCHITECTURE §10.2).
    dap: Mutex<Option<Arc<crate::dap::DapHub>>>,
    /// How to relaunch this session's app (Swift Quick Relaunch, RD §4.7); set by [`crate::DebugService`].
    relauncher: Mutex<Option<Arc<dyn crate::Relaunch>>>,
    facts: Mutex<SessionFacts>,
    /// Held while a record is computed and written, so the last write is never an older view.
    persisting: Mutex<()>,
}

/// What tells a session that its process ended when no debugger holds it.
pub(crate) struct ExitSignals {
    pub exit: Option<crate::launch::ExitWatch>,
    pub stdio_done: Option<tokio::sync::oneshot::Receiver<()>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A handle on a live (or ended) session. Cheap to clone.
#[derive(Clone)]
pub struct SessionHandle {
    pub(crate) inner: Arc<Session>,
}

impl std::fmt::Debug for SessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionHandle").field("id", &self.inner.id).field("state", &self.state()).finish()
    }
}

/// What a new session is.
pub(crate) struct SessionInit {
    pub id: SessionId,
    pub kind: SessionKind,
    pub bundle_id: String,
    pub project: Option<String>,
    pub lease: SessionLease,
    pub attach_mode: Option<AttachMode>,
    pub state_dir: Option<PathBuf>,
    pub log_capacity: usize,
    pub facts: SessionFacts,
}

impl SessionHandle {
    pub(crate) fn create(init: SessionInit) -> Self {
        let dir = init.state_dir.as_ref().map(|d| d.join("sessions").join(init.id.as_str()));
        let registry = init.state_dir.as_ref().map(|d| d.join("sessions.json"));
        let logs = LogView::new(init.log_capacity);
        if let Some(dir) = &dir {
            if let Err(e) = crate::fsutil::private_dir(dir) {
                tracing::warn!(error = %e, "cannot create the session directory");
            } else if let Err(e) = logs.persist_to(&dir.join("output.log")) {
                tracing::warn!(error = %e, "cannot open output.log");
            }
            if let Some(sessions) = dir.parent() {
                rotate(sessions, KEEP_SESSIONS);
            }
        }
        let (state, _) = watch::channel(SessionState::Preparing);
        let (debugger, _) = watch::channel(DebuggerState::None);
        let (events, _) = broadcast::channel(256);
        let s = Session {
            id: init.id,
            kind: init.kind,
            bundle_id: init.bundle_id,
            udid: Mutex::new(String::new()),
            project: init.project,
            lease: init.lease,
            attach_mode: init.attach_mode,
            started_at: logs::now_rfc3339(),
            pid: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            state,
            debugger,
            ended: Mutex::new(None),
            kill_requested: AtomicBool::new(false),
            events,
            logs,
            control: tokio::sync::Mutex::new(Control::None),
            dir,
            registry,
            keep: Mutex::new(Vec::new()),
            exit_watch: Mutex::new(None),
            dap: Mutex::new(None),
            relauncher: Mutex::new(None),
            facts: Mutex::new(init.facts),
            persisting: Mutex::new(()),
        };
        let h = Self { inner: Arc::new(s) };
        h.persist();
        h
    }

    pub fn id(&self) -> &SessionId {
        &self.inner.id
    }

    pub fn state(&self) -> SessionState {
        if let Some(e) = lock(&self.inner.ended).as_ref() {
            return e.state.clone();
        }
        self.inner.state.borrow().clone()
    }

    pub fn debugger(&self) -> DebuggerState {
        *self.inner.debugger.borrow()
    }

    pub fn pid(&self) -> Option<u32> {
        match self.inner.pid.load(Ordering::SeqCst) {
            0 => None,
            p => Some(p),
        }
    }

    pub fn lease(&self) -> SessionLease {
        self.inner.lease
    }

    pub fn kind(&self) -> SessionKind {
        self.inner.kind
    }

    pub fn bundle_id(&self) -> &str {
        &self.inner.bundle_id
    }

    /// Whether the session has ended.
    pub fn is_ended(&self) -> bool {
        lock(&self.inner.ended).is_some()
    }

    /// Session events: states, debugger changes and the end.
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.inner.events.subscribe()
    }

    /// A receiver of state changes.
    pub fn watch_state(&self) -> watch::Receiver<SessionState> {
        self.inner.state.subscribe()
    }

    /// The merged stdout and os_trace output (`run.output`).
    pub fn logs(&self) -> LogView {
        self.inner.logs.clone()
    }

    /// Why the session failed, when it ended with an error Russet knows (`run.exited.error`).
    pub fn error(&self) -> Option<SessionError> {
        lock(&self.inner.ended).as_ref().and_then(|e| e.error.clone())
    }

    /// RD §21's facts the session has gathered so far.
    pub fn facts(&self) -> SessionFacts {
        lock(&self.inner.facts).clone()
    }

    /// The wire summary (ARCHITECTURE §9.5).
    pub fn summary(&self) -> SessionSummary {
        let ended = lock(&self.inner.ended);
        let state = match ended.as_ref() {
            Some(e) => e.state.wire(),
            None => self.inner.state.borrow().wire(),
        };
        SessionSummary {
            session_id: self.inner.id.clone(),
            kind: self.inner.kind,
            bundle_id: self.inner.bundle_id.clone(),
            udid: lock(&self.inner.udid).clone(),
            pid: self.pid(),
            project: self.inner.project.clone(),
            state,
            lease: self.inner.lease,
            debugger: self.debugger(),
            attach_mode: self.inner.attach_mode,
            started_at: self.inner.started_at.clone(),
            ended_at: ended.as_ref().map(|e| e.at.clone()),
            exit: ended.as_ref().and_then(|e| e.exit.clone()),
        }
    }

    /// Waits until the session ends; returns its final state.
    /// The final state is announced only once `session.json` records it, so a waiter that reads the file
    /// afterwards sees the end.
    pub async fn wait(&self) -> SessionState {
        let mut rx = self.inner.state.subscribe();
        loop {
            let s = rx.borrow_and_update().clone();
            if s.is_final() {
                return s;
            }
            if rx.changed().await.is_err() {
                return self.state();
            }
        }
    }

    /// Waits up to `limit` for `pred` on the state, or for the end; returns the state then.
    pub async fn wait_for(&self, limit: Duration, pred: impl Fn(&SessionState) -> bool) -> SessionState {
        let mut rx = self.inner.state.subscribe();
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let s = rx.borrow_and_update().clone();
            if pred(&s) || s.is_final() {
                return s;
            }
            match tokio::time::timeout_at(deadline, rx.changed()).await {
                Ok(Ok(())) => {}
                _ => return self.state(),
            }
        }
    }

    /// Ends the session: `Kill` stops the app; `Detach` leaves it running and ends Russet's tracking (the
    /// session's state becomes `exited` without exit details).
    pub async fn terminate(&self, how: TerminateMode) -> Result<()> {
        if self.is_ended() {
            return Ok(());
        }
        if how == TerminateMode::Kill {
            self.inner.kill_requested.store(true, Ordering::SeqCst);
        }
        let control = {
            let mut slot = self.inner.control.lock().await;
            let control = std::mem::replace(&mut *slot, Control::None);
            if matches!(control, Control::None) {
                // Not launched yet: the session ends now, under the lock, so the launch's `set_control` sees the
                // end and stops the app it started.
                self.end_untracked();
                return Ok(());
            }
            control
        };
        match control {
            Control::Broker { handle, process } => {
                handle.terminate(how).await?;
                match (how, handle.state()) {
                    (_, BrokerState::Exited { code, signal }) => self.end_exited(code, signal),
                    (TerminateMode::Detach, _) => self.end_untracked(),
                    (TerminateMode::Kill, BrokerState::Lost) => {
                        self.end_failed(SessionState::Lost, &crate::errors::session_lost());
                    }
                    // The debugger had detached already: the app runs on its own, so the device kills it.
                    (TerminateMode::Kill, BrokerState::Detached) => {
                        process.kill().await?;
                        self.end_exited(None, Some(SIGKILL));
                    }
                    (TerminateMode::Kill, _) => self.end_exited(None, Some(SIGKILL)),
                }
            }
            Control::Process(process) => match how {
                TerminateMode::Kill => {
                    process.kill().await?;
                    self.end_exited(None, Some(SIGKILL));
                }
                TerminateMode::Detach => self.end_untracked(),
            },
            Control::None => {}
        }
        Ok(())
    }

    /// `run.hotReload`: Swift Quick Relaunch (W11); Flutter hot reload lands with W18. Returns the timing.
    pub async fn hot_reload(&self, _full: bool) -> Result<HotReloadResult> {
        let start = std::time::Instant::now();
        let (_pid, _) = self.relaunch().await?;
        Ok(HotReloadResult {
            supported: true,
            kind: "restart".to_owned(),
            ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            message: Some("Swift Quick Relaunch (restart); Flutter hot reload lands with W18".to_owned()),
        })
    }

    /// The per-session DAP hub, when one is attached.
    pub fn dap_hub(&self) -> Option<Arc<crate::dap::DapHub>> {
        lock(&self.inner.dap).clone()
    }

    /// Attaches a DAP hub to this session (`debug.prepare {frontend: dap}`).
    pub fn set_dap_hub(&self, hub: Arc<crate::dap::DapHub>) {
        *lock(&self.inner.dap) = Some(hub);
    }

    /// Sets how this session relaunches its app (Swift Quick Relaunch).
    pub fn set_relauncher(&self, r: Arc<dyn crate::Relaunch>) {
        *lock(&self.inner.relauncher) = Some(r);
    }

    /// Swift Quick Relaunch: relaunch the app and return `(pid, ms)`. The session keeps its id.
    pub async fn relaunch(&self) -> Result<(u32, u64)> {
        if self.is_ended() {
            return Err(crate::errors::session_not_found(self.inner.id.as_str())
                .with_hint("This session has ended; start a new one with `russet run` or `russet debug`."));
        }
        let relauncher = lock(&self.inner.relauncher).clone().ok_or_else(|| {
            russet_core::Error::new(
                russet_core::error::codes::core::NOT_IMPLEMENTED,
                "this session cannot be relaunched",
            )
            .with_hint("Relaunch is for Swift `run`/`debug` sessions; Flutter hot reload lands with W18.")
        })?;
        let start = std::time::Instant::now();
        let pid = relauncher.relaunch(self).await?;
        Ok((pid, u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)))
    }

    /// This session's launch generation. An exit watcher captures it and ends the session only while it still
    /// matches, so a relaunch's kill of the old process cannot end the new one.
    pub(crate) fn generation(&self) -> u32 {
        self.inner.generation.load(Ordering::SeqCst)
    }

    /// Bumps the generation (a new launch is starting) and returns the new value.
    pub(crate) fn next_generation(&self) -> u32 {
        self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Replaces the session's running process (Quick Relaunch): kill the old control, install the new one, set
    /// `pid` and the state. The generation must already have been bumped (so the old watcher stands down). The
    /// session id and subscriptions are unchanged.
    pub(crate) async fn swap_process(&self, control: Control, pid: u32, state: SessionState) {
        let old = std::mem::replace(&mut *self.inner.control.lock().await, control);
        stop_control(old).await;
        self.inner.pid.store(pid, Ordering::SeqCst);
        self.set_state(state);
        self.persist();
    }

    /// Kills an owned session `grace` after `owner_gone` resolves (ARCHITECTURE §6.3: owner gone + 2 s). A
    /// detached session ignores it.
    pub fn end_with_owner<F>(&self, owner_gone: F, grace: Duration)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        if self.inner.lease != SessionLease::Owned {
            return;
        }
        let h = self.clone();
        tokio::spawn(async move {
            let ended = h.wait();
            tokio::select! {
                _ = ended => {}
                () = owner_gone => {
                    tokio::time::sleep(grace).await;
                    if !h.is_ended() {
                        tracing::info!(session = %h.id(), "owner gone: ending the owned session");
                        let _ = h.terminate(TerminateMode::Kill).await;
                    }
                }
            }
        });
    }

    // ---- internal transitions ------------------------------------------------------------------------------

    pub(crate) fn set_udid(&self, udid: &str) {
        *lock(&self.inner.udid) = udid.to_owned();
        self.persist();
    }

    pub(crate) fn set_pid(&self, pid: u32) {
        self.inner.pid.store(pid, Ordering::SeqCst);
        self.persist();
    }

    /// Records facts for `session.json`.
    pub(crate) fn update_facts(&self, f: impl FnOnce(&mut SessionFacts)) {
        f(&mut lock(&self.inner.facts));
        self.persist();
    }

    pub(crate) fn set_state(&self, s: SessionState) {
        let mut previous = None;
        let changed = {
            // Under the `ended` lock, so a concurrent end() can never be overwritten.
            let ended = lock(&self.inner.ended);
            if ended.is_some() {
                return;
            }
            self.inner.state.send_if_modified(|cur| {
                if *cur == s {
                    false
                } else {
                    previous = Some(std::mem::replace(cur, s.clone()));
                    true
                }
            })
        };
        if changed {
            let _ = self.inner.events.send(SessionEvent::State(s.clone()));
            // Stepping toggles running and stopped many times; the files only need the milestones.
            let toggle = |x: &SessionState| matches!(x, SessionState::Running | SessionState::Stopped);
            if !(toggle(&s) && previous.as_ref().is_some_and(toggle)) {
                self.persist();
            }
        }
    }

    pub(crate) fn set_debugger(&self, d: DebuggerState) {
        let changed = self.inner.debugger.send_if_modified(|cur| {
            if *cur == d {
                false
            } else {
                *cur = d;
                true
            }
        });
        if changed {
            let _ = self.inner.events.send(SessionEvent::Debugger(d));
            let text = match d {
                DebuggerState::Attached => "debugger attached",
                DebuggerState::Detached => "debugger detached; the app keeps running",
                DebuggerState::None => "debugger gone",
            };
            self.inner.logs.push(logs::russet_entry(LogLevel::Info, text));
        }
    }

    /// Stores how to stop the session; hands the control back when the session already ended (a stop that raced
    /// the launch), so the caller stops the process it just started.
    pub(crate) async fn set_control(&self, c: Control) -> std::result::Result<(), Control> {
        let mut slot = self.inner.control.lock().await;
        if self.is_ended() {
            return Err(c);
        }
        *slot = c;
        Ok(())
    }

    /// Stops whatever the session controls without touching its state (after a failed preparation, which ends
    /// the session as `failed` first).
    pub(crate) async fn abort(&self) {
        let control = std::mem::replace(&mut *self.inner.control.lock().await, Control::None);
        stop_control(control).await;
    }

    /// Keeps `thing` until the session ends. Once it has ended (a stop that raced the launch), `thing` is dropped
    /// at once: under the `ended` lock, so end()'s clearing never misses it.
    pub(crate) fn keep_alive(&self, thing: Box<dyn std::any::Any + Send>) {
        let ended = lock(&self.inner.ended);
        if ended.is_none() {
            lock(&self.inner.keep).push(thing);
            return;
        }
        drop(ended);
        drop(thing);
    }

    pub(crate) fn kill_requested(&self) -> bool {
        self.inner.kill_requested.load(Ordering::SeqCst)
    }

    /// Records that the process was killed on purpose (by Russet or by the debugger's `k`).
    pub(crate) fn mark_kill_requested(&self) {
        self.inner.kill_requested.store(true, Ordering::SeqCst);
    }

    pub(crate) fn stash_exit_signals(&self, w: ExitSignals) {
        *lock(&self.inner.exit_watch) = Some(w);
    }

    pub(crate) fn take_exit_signals(&self) -> ExitSignals {
        lock(&self.inner.exit_watch).take().unwrap_or(ExitSignals { exit: None, stdio_done: None })
    }

    /// The process exited: `exited`, or `crashed` for a signal Russet did not send.
    pub(crate) fn end_exited(&self, code: Option<i32>, signal: Option<i32>) {
        let crashed = signal.is_some_and(|s| !(s == SIGKILL && self.kill_requested()) && s != 15 && s != 2);
        let exit = SessionExit { code, signal, crash_id: None };
        let state = if crashed {
            SessionState::Crashed { crash_id: None }
        } else {
            SessionState::Exited { code: code.unwrap_or(-1) }
        };
        self.end(state, Some(exit));
    }

    /// Russet stops tracking a process it leaves running.
    pub(crate) fn end_untracked(&self) {
        self.end(SessionState::Exited { code: -1 }, None);
    }

    /// Ends the session as `failed` or `lost` because of `e`, which `run.exited`, `session.json` and the output
    /// (a `russet` entry, level `error`, `category` = the code's name) report.
    pub(crate) fn end_failed(&self, state: SessionState, e: &russet_core::Error) {
        let error = SessionError {
            code: e.code().number(),
            russet_code: e.code().as_str().to_owned(),
            message: e.message().to_owned(),
            hint: e.hint().map(str::to_owned),
        };
        if !self.is_ended() {
            self.inner.logs.push(logs::error_entry(&error));
        }
        self.end_with(state, None, Some(error));
    }

    pub(crate) fn end(&self, state: SessionState, exit: Option<SessionExit>) {
        self.end_with(state, exit, None);
    }

    /// Ends the session. The end is decided under the `ended` lock (later transitions are ignored), then
    /// recorded (`session.json`, `sessions.json`), and only then announced to waiters and subscribers.
    fn end_with(&self, state: SessionState, exit: Option<SessionExit>, error: Option<SessionError>) {
        {
            let mut ended = lock(&self.inner.ended);
            if ended.is_some() {
                return;
            }
            *ended =
                Some(Ended { state: state.clone(), at: logs::now_rfc3339(), exit: exit.clone(), error: error.clone() });
        }
        lock(&self.inner.keep).clear();
        self.persist();
        self.inner.state.send_replace(state.clone());
        let _ = self.inner.events.send(SessionEvent::State(state.clone()));
        let _ = self.inner.events.send(SessionEvent::Ended { state: state.wire(), exit, error });
    }

    /// Writes `session.json` and updates `sessions.json`.
    pub(crate) fn persist(&self) {
        let _one_at_a_time = lock(&self.inner.persisting);
        let summary = self.summary();
        if let Some(dir) = &self.inner.dir {
            let record = session_record(&summary, &self.facts(), self.error());
            if let Ok(bytes) = serde_json::to_vec_pretty(&record) {
                let _ = crate::fsutil::atomic_write(&dir.join("session.json"), &bytes);
            }
        }
        if let Some(path) = &self.inner.registry {
            update_registry(path, &summary);
        }
    }
}

/// `session.json` (RD §21): `schema`, `sessionId`, `kind`, `device`, `app`, `pid`, `launch`, `broker`, `jit` (W18),
/// `lldb`, `startedAt`, `endedAt` and `outcome`, plus the summary's `project`, `state`, `lease`, `debugger` and
/// `attachMode`. Unknown values are `null`; `outcome` is `null` until the session ends, then `{state, exit?,
/// error?}`.
fn session_record(s: &SessionSummary, f: &SessionFacts, error: Option<SessionError>) -> serde_json::Value {
    let outcome = s.ended_at.as_ref().map(|_| {
        let mut o = json!({"state": s.state});
        if let Some(exit) = &s.exit {
            o["exit"] = json!(exit);
        }
        if let Some(e) = &error {
            o["error"] = json!(e);
        }
        o
    });
    json!({
        "schema": 1,
        "sessionId": s.session_id,
        "kind": s.kind,
        "device": {
            "udid": s.udid,
            "productType": f.device.product_type,
            "productVersion": f.device.product_version,
            "buildVersion": f.device.build_version,
            "transport": f.device.transport,
        },
        "app": {
            "bundleId": s.bundle_id,
            "devicePath": f.app_device_path,
            "executable": f.app_executable,
            "uuid": f.app_uuid,
        },
        "pid": s.pid,
        "launch": f.launch,
        "broker": f.lldb_url.as_ref().map(|url| json!({"lldbUrl": url})),
        "jit": null,
        "lldb": f.lldb.as_ref().map(|l| json!({"flavor": l.flavor, "version": l.version, "python": l.python})),
        "project": s.project,
        "state": s.state,
        "lease": s.lease,
        "debugger": s.debugger,
        "attachMode": s.attach_mode,
        "startedAt": s.started_at,
        "endedAt": s.ended_at,
        "outcome": outcome,
    })
}

impl SessionState {
    /// The wire value (ARCHITECTURE §9.5).
    pub fn wire(&self) -> WireState {
        match self {
            Self::Preparing => WireState::Preparing,
            Self::Building => WireState::Building,
            Self::Installing => WireState::Installing,
            Self::Launching => WireState::Launching,
            Self::Attaching => WireState::Attaching,
            Self::Arming => WireState::Arming,
            Self::Ready => WireState::Ready,
            Self::Running => WireState::Running,
            Self::Stopped => WireState::Stopped,
            Self::Exited { .. } => WireState::Exited,
            Self::Crashed { .. } => WireState::Crashed,
            Self::Lost => WireState::Lost,
            Self::Failed => WireState::Failed,
        }
    }

    /// Whether the session is over.
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Exited { .. } | Self::Crashed { .. } | Self::Lost | Self::Failed)
    }
}

static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

/// `state/sessions.json`: `{schema, sessions: [summary…]}` of the sessions that have not ended.
fn update_registry(path: &Path, s: &SessionSummary) {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut list: Vec<serde_json::Value> = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("sessions").and_then(|s| s.as_array()).cloned())
        .unwrap_or_default();
    list.retain(|v| v.get("sessionId").and_then(|i| i.as_str()) != Some(s.session_id.as_str()));
    if s.ended_at.is_none()
        && let Ok(v) = serde_json::to_value(s)
    {
        list.push(v);
    }
    let doc = json!({"schema": 1, "sessions": list});
    if let Ok(bytes) = serde_json::to_vec_pretty(&doc) {
        let _ = crate::fsutil::atomic_write(path, &bytes);
    }
}

/// Keeps the newest `keep` session directories. A session that has not ended (its `session.json` has no
/// `endedAt`) is kept unless it is older than a week (a daemon crash can leave such a record behind).
fn rotate(sessions: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(sessions) else { return };
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = rd
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if dirs.len() <= keep {
        return;
    }
    dirs.sort();
    let mut excess = dirs.len() - keep;
    let week = Duration::from_secs(7 * 24 * 3600);
    for (modified, d) in dirs {
        if excess == 0 {
            break;
        }
        let live = std::fs::read(d.join("session.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .is_some_and(|v| v.get("endedAt").is_some_and(serde_json::Value::is_null));
        let recent = modified.elapsed().is_ok_and(|age| age < week);
        if live && recent {
            continue;
        }
        let _ = std::fs::remove_dir_all(d);
        excess -= 1;
    }
}

/// A fresh `ses_…` id: a ULID (48-bit millisecond time, 80 random bits, Crockford base32).
///
/// russet-core's `SessionId::generate` is W2's (CORE-02); until it lands this builds the same shape and goes
/// through serde, the only public constructor of the newtype.
pub fn new_session_id() -> SessionId {
    prefixed_ulid(SessionId::PREFIX)
}

/// A prefixed ULID id of any of russet-core's id newtypes (`ses_`, `sub_`, …), built through serde.
pub fn prefixed_ulid<T: serde::de::DeserializeOwned>(prefix: &str) -> T {
    let text = format!("{prefix}{}", ulid());
    match serde_json::from_value(serde_json::Value::String(text)) {
        Ok(id) => id,
        Err(_) => unreachable!("an id newtype deserializes from any string"),
    }
}

/// A ULID: 48-bit millisecond time and 80 random bits in Crockford base32 (26 characters).
pub fn ulid() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let ms = u128::try_from(logs::now_epoch_ms()).unwrap_or(0) & ((1 << 48) - 1);
    let mut random = 0u128;
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u64(i);
        h.write_u128(ms);
        random = (random << 64) | u128::from(h.finish());
    }
    let value = (ms << 80) | (random & ((1 << 80) - 1));
    (0..26).rev().map(|i| char::from(ALPHABET[((value >> (i * 5)) & 0x1f) as usize])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init(state_dir: Option<PathBuf>) -> SessionInit {
        SessionInit {
            id: new_session_id(),
            kind: SessionKind::Debug,
            bundle_id: "dev.russet.test.probe".into(),
            project: Some("/home/u/RussetProbe".into()),
            lease: SessionLease::Detached,
            attach_mode: Some(AttachMode::Broker),
            state_dir,
            log_capacity: 100,
            facts: SessionFacts::default(),
        }
    }

    #[test]
    fn session_ids_are_prefixed_ulids() {
        let a = new_session_id();
        let b = new_session_id();
        assert_ne!(a, b);
        let s = a.as_str();
        assert_eq!(s.len(), 30);
        assert!(s.starts_with("ses_"));
        assert!(s[4..].bytes().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
        assert!(!s[4..].contains(['I', 'L', 'O', 'U']));
    }

    #[tokio::test]
    async fn states_debugger_and_the_end_reach_the_summary_and_the_files() {
        let dir = std::env::temp_dir().join(format!("russet-session-{}", std::process::id()));
        let h = SessionHandle::create(init(Some(dir.clone())));
        let mut events = h.subscribe();
        h.set_udid("00008101-0000000000000000");
        h.set_pid(932);
        h.set_state(SessionState::Running);
        h.set_debugger(DebuggerState::Detached);
        let s = h.summary();
        assert_eq!(
            (s.state, s.debugger, s.lease, s.pid),
            (WireState::Running, DebuggerState::Detached, SessionLease::Detached, Some(932))
        );
        assert!(s.ended_at.is_none());
        let registry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("sessions.json")).unwrap_or_default()).unwrap_or_default();
        assert_eq!(registry["sessions"].as_array().map(Vec::len), Some(1));
        h.end_exited(Some(0), None);
        assert!(h.is_ended());
        let s = h.summary();
        assert_eq!(s.state, WireState::Exited);
        assert_eq!(s.exit, Some(SessionExit { code: Some(0), signal: None, crash_id: None }));
        let record: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("sessions").join(h.id().as_str()).join("session.json")).unwrap_or_default(),
        )
        .unwrap_or_default();
        assert_eq!(record["state"], "exited");
        assert_eq!(record["schema"], 1);
        let registry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("sessions.json")).unwrap_or_default()).unwrap_or_default();
        assert_eq!(registry["sessions"].as_array().map(Vec::len), Some(0));
        // Events: running, debugger, then the end.
        let mut seen = Vec::new();
        while let Ok(e) = events.try_recv() {
            seen.push(e);
        }
        assert!(seen.contains(&SessionEvent::State(SessionState::Running)));
        assert!(seen.contains(&SessionEvent::Debugger(DebuggerState::Detached)));
        assert!(matches!(seen.last(), Some(SessionEvent::Ended { state: WireState::Exited, .. })));
        // A further transition is ignored.
        h.set_state(SessionState::Running);
        assert_eq!(h.summary().state, WireState::Exited);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signals_russet_did_not_send_are_crashes() {
        let h = SessionHandle::create(init(None));
        h.end_exited(None, Some(11));
        assert_eq!(h.summary().state, WireState::Crashed);
        let h = SessionHandle::create(init(None));
        h.inner.kill_requested.store(true, Ordering::SeqCst);
        h.end_exited(None, Some(9));
        assert_eq!(h.summary().state, WireState::Exited);
        assert_eq!(h.summary().exit.and_then(|e| e.signal), Some(9));
    }

    #[test]
    fn old_session_directories_rotate() {
        let dir = std::env::temp_dir().join(format!("russet-rotate-{}", std::process::id()));
        for i in 0..5 {
            let _ = std::fs::create_dir_all(dir.join(format!("ses_{i}")));
            std::thread::sleep(Duration::from_millis(5));
        }
        // ses_1 is live: it stays even though it is old.
        let _ = std::fs::write(dir.join("ses_1/session.json"), br#"{"endedAt": null}"#);
        rotate(&dir, 3);
        let left = std::fs::read_dir(&dir).map(|r| r.count()).unwrap_or(0);
        assert_eq!(left, 3);
        assert!(dir.join("ses_4").exists() && dir.join("ses_1").exists());
        assert!(!dir.join("ses_0").exists() && !dir.join("ses_2").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
