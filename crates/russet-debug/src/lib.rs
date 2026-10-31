// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Run and debug sessions (ARCHITECTURE §5.3 `russet-debug`; RD §17.2): launch strategies, the gdb-remote
//! broker, LLDB plans, the run-output view and the session registry; later the JIT servicer, the DAP hub,
//! symbols, crashes and Flutter run/debug.
//!
//! - [`DebugService::run`] launches without a debugger (RD §4.1).
//! - [`DebugService::prepare_debug`] launches suspended, has the [`broker`] `vAttach` before dyld runs, closes
//!   the launch channel, serves the broker endpoint and writes the Python-free `attach.lldb` (RD §4.2,
//!   ARCHITECTURE §17.1). `attachMode: legacy-async` serves a plain forward instead and leaves the attach to
//!   LLDB (the M1 sequence) [D 38].
//! - [`SessionState`] and [`SessionSummary`] convert to the wire shapes of ARCHITECTURE §9.5 (state, lease,
//!   debugger) [D 84].
//!
//! Owners: W7 (Wave 1), W11 (Wave 2), W18 (Wave 3); W30 owns [`wifi`].

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, codes};
use russet_core::ids::SessionId;
use russet_core::progress::ProgressSink;
use russet_device::{AppFilter, DeviceHandle, DeviceManager, DeviceSelector, InstallOptions, LaunchRequest, Udid};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{broadcast, mpsc};

pub mod broker;
pub mod crash;
pub mod dap;
pub mod errors;
pub mod flutter;
mod fsutil;
pub mod jit;
pub mod launch;
pub mod lldb;
pub mod logs;
pub mod macho;
pub mod session;
pub mod symbols;
pub mod wifi;

pub use russet_protocol::common::{
    AttachMode, DebuggerState, LogEntry, SessionExit, SessionKind, SessionLease, SessionState as WireSessionState,
    SessionSummary,
};
pub use session::{DeviceFacts, LldbFacts, SessionFacts, SessionHandle, new_session_id};

/// `async_trait`, for implementations of [`DeviceServices`] and for russetd's pipeline ports: russetd's manifest
/// is a seam file without the crate, and the macro's expansion names only `core` and `std` items.
pub use async_trait::async_trait;

use crate::broker::{Broker, BrokerEvent, EndpointSpec};
use crate::launch::{InstalledAppInfo, LaunchChannel, LaunchSpec, LaunchStrategy, Launched};
use crate::lldb::{AttachPlan, InitSettings, LldbPlatform};
use crate::session::{Control, ExitSignals, ProcessRef, SessionInit};

/// A byte stream to debugserver or to LLDB.
pub trait DebugStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> DebugStream for T {}
/// A boxed [`DebugStream`].
pub type BoxStream = Box<dyn DebugStream>;

/// What russet-debug needs from the device plane (RD §17.3). Implemented here for
/// [`russet_device::DeviceManager`], and by russetd's `DeviceSlot`, which overrides
/// [`DeviceServices::manager`] to start the manager on first use and so gets every other method [D 83].
///
/// Every method has a default built on [`DeviceServices::device`], which uses [`DeviceServices::manager`]:
/// test doubles override the methods directly.
#[async_trait]
pub trait DeviceServices: Send + Sync {
    /// The device manager. The default reports that this implementation has none.
    async fn manager(&self) -> Result<Arc<DeviceManager>> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "this device-services implementation has no device manager")
            .with_hint("russetd's DeviceSlot provides the manager (W2)."))
    }

    /// A handle on the device with this UDID.
    async fn device(&self, udid: &Udid) -> Result<DeviceHandle> {
        self.manager().await?.get(&DeviceSelector::Udid(udid.clone()))
    }

    /// The UDID a selector names (`Only`: the one ready device).
    async fn resolve(&self, sel: &DeviceSelector) -> Result<Udid> {
        match sel {
            DeviceSelector::Udid(u) => Ok(u.clone()),
            DeviceSelector::Only => only_device(self.manager().await?.as_ref()),
            DeviceSelector::Alias(a) => Err(Error::new(
                codes::core::INVALID_ARGUMENT,
                format!("device aliases are resolved by the device plane; `{a}` is not known here"),
            )
            .with_hint("Pass the device's UDID.")),
        }
    }

    /// The installed app for `bundle_id` (exact, or xtool's `XTL-<prefix>.<id>`).
    async fn installed_app(&self, udid: &Udid, bundle_id: &str) -> Result<Option<InstalledAppInfo>> {
        let apps = self.device(udid).await?.apps(AppFilter::default()).await?;
        let infos: Vec<InstalledAppInfo> = apps
            .into_iter()
            .map(|a| InstalledAppInfo { bundle_id: a.bundle_id, path: None, executable: None })
            .collect();
        Ok(launch::resolve_bundle(&infos, bundle_id).cloned())
    }

    /// Installs a `.app` or `.ipa`.
    async fn install(&self, udid: &Udid, app: &Path, progress: &dyn ProgressSink) -> Result<InstalledAppInfo> {
        let installed = self.device(udid).await?.install(app, InstallOptions::default(), progress).await?;
        Ok(InstalledAppInfo { bundle_id: installed.bundle_id, path: None, executable: None })
    }

    /// Launches through CoreDevice (DVT fallback), keeping the launch channel open in [`Launched::channel`].
    async fn launch(&self, udid: &Udid, spec: &LaunchSpec) -> Result<Launched> {
        let d = self.device(udid).await?;
        // F0's request has only these fields; W4's F2 adds more (the CoreDevice/DVT choice), whose defaults apply
        // until they are mapped here.
        #[allow(clippy::needless_update)]
        let request = LaunchRequest {
            bundle_id: spec.bundle_id.clone(),
            args: spec.args.clone(),
            env: spec.env.clone(),
            suspended: spec.suspended,
            kill_existing: spec.kill_existing,
            console: spec.console,
            ..LaunchRequest::default()
        };
        let process = d.launch(request).await?;
        let mut launched = Launched::pid_only(process.pid);
        launched.executable = Some(process.executable.clone());
        launched.strategy = spec.strategy;
        launched.entries = process.output.clone().map(|hub| console_entries(hub, process.exit.clone()));
        let mut exit = process.exit.clone();
        launched.exit = Some(Box::pin(async move {
            let info = exit.wait().await;
            launch::ProcessExit { code: info.code, signal: info.signal }
        }));
        // The LaunchedProcess holds the launch channel; it closes when the channel guard drops.
        launched.channel = Some(LaunchChannel::new(process));
        Ok(launched)
    }

    /// A fresh connection to the device's debugserver (`com.apple.internal.dt.remote.debugproxy`, RSD).
    async fn connect_debugserver(&self, udid: &Udid) -> Result<BoxStream> {
        let s = self.device(udid).await?.connect_rsd(broker::DEBUGPROXY_SERVICE).await?;
        Ok(Box::new(s))
    }

    /// Sends `sig` to `pid`.
    async fn signal(&self, udid: &Udid, pid: u32, sig: i32) -> Result<()> {
        self.device(udid).await?.signal(pid, sig).await
    }

    /// The process's os_trace entries, when the device plane can stream them.
    async fn os_log(&self, _udid: &Udid, _pid: u32) -> Result<Option<mpsc::Receiver<LogEntry>>> {
        Ok(None)
    }

    /// What `session.json` records about the device (RD §21: product type, iOS version and build, transport).
    /// `None` until the device plane maps its `DeviceInfo` here (W4's F2 fields).
    async fn describe(&self, _udid: &Udid) -> Result<Option<DeviceFacts>> {
        Ok(None)
    }

    /// Pulls crash reports (`.ips` bytes) off the device for `crash.fetch` (RD §13, DEV-15). The default is
    /// unsupported; the device plane (W10) overrides it (crashreportmover flush + AFC copy). Device check DC-10.
    async fn fetch_crash_reports(
        &self,
        _udid: &Udid,
        _bundle_id: Option<&str>,
        _since: Option<&str>,
    ) -> Result<Vec<Vec<u8>>> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "fetching crash reports off the device lands with W10 (DEV-15)")
            .with_hint("Crash reports captured on abnormal exit still reach the store; see `russet crash list`."))
    }

    /// Lists the device's symbol files and their sizes for `debug.symbols.plan`/`fetch`
    /// (`com.apple.dt.remoteFetchSymbols`, RD §11.2, DEV-17). Unsupported by default; the device plane overrides
    /// it. Device check DC-09.
    async fn list_symbol_files(&self, _udid: &Udid) -> Result<Vec<crate::symbols::DeviceSymbolFile>> {
        Err(Error::new(codes::debug::SYMBOLS_SERVICE, "the device symbol-file service is not wired yet (DEV-17)")
            .with_hint("Device symbol fetch needs owner approval (Q13) and tier GREEN; see docs/lab/queue.md DC-09."))
    }
}

/// The console hub of a launch as session entries: the backlog, then live lines until the process exits (the
/// lines that arrived before the exit notice are still delivered).
fn console_entries(
    hub: Arc<russet_device::process::OutputHub>,
    mut exit: russet_device::ExitWatcher,
) -> launch::ConsoleEntries {
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(async move {
        let (backlog, mut live) = hub.subscribe();
        for e in backlog {
            if tx.send(e).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                r = live.recv() => match r {
                    Ok(e) => {
                        if tx.send(e).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = exit.wait() => {
                    while let Ok(e) = live.try_recv() {
                        if tx.send(e).await.is_err() {
                            return;
                        }
                    }
                    return;
                }
            }
        }
    });
    rx
}

fn only_device(m: &DeviceManager) -> Result<Udid> {
    let devices = m.devices();
    match devices.as_slice() {
        [one] => Ok(one.udid.clone()),
        [] => Err(Error::new(codes::device::NOT_FOUND, "no device is connected")
            .with_hint("Connect the iPhone with USB and run `russet device list`.")),
        _ => Err(Error::new(codes::core::INVALID_ARGUMENT, "more than one device is connected")
            .with_hint("Choose one with `--device <udid>`.")),
    }
}

#[async_trait]
impl DeviceServices for DeviceManager {
    async fn device(&self, udid: &Udid) -> Result<DeviceHandle> {
        self.get(&DeviceSelector::Udid(udid.clone()))
    }

    async fn resolve(&self, sel: &DeviceSelector) -> Result<Udid> {
        match sel {
            DeviceSelector::Udid(u) => Ok(u.clone()),
            DeviceSelector::Only => only_device(self),
            DeviceSelector::Alias(a) => Err(Error::new(
                codes::core::INVALID_ARGUMENT,
                format!("device aliases are resolved by the device plane; `{a}` is not known here"),
            )
            .with_hint("Pass the device's UDID.")),
        }
    }
}

/// How LLDB reaches the broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerTransport {
    /// A 0600 Unix socket under `runtime/sessions/<sid>/` (Linux).
    Unix,
    /// Loopback TCP (Windows, or on request).
    Tcp,
}

/// Debug-plane configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugConfig {
    /// `runtime/`: broker sockets go to `runtime/sessions/<sid>/gdb.sock`.
    pub runtime_dir: PathBuf,
    /// `state/`: `sessions.json` and `sessions/<sid>/`. `None` keeps sessions in memory only.
    pub state_dir: Option<PathBuf>,
    pub broker: broker::BrokerOptions,
    pub broker_transport: BrokerTransport,
    pub log_capacity: usize,
    /// How long an owned session outlives its owner (ARCHITECTURE §6.3).
    pub lease_grace: Duration,
    /// Ended sessions kept in memory for `run.list`.
    pub keep_ended: usize,
}

impl DebugConfig {
    /// The configuration for Russet's directories (ARCHITECTURE §15).
    pub fn from_dirs(dirs: &RussetDirs) -> Self {
        Self { runtime_dir: dirs.runtime.clone(), state_dir: Some(dirs.state.clone()), ..Self::default() }
    }
}

/// The runtime directory when Russet's directories are not known (tests, the doctor without `RUSSET_HOME` or
/// `XDG_RUNTIME_DIR`): per user, so two users of one host never share it.
pub fn fallback_runtime_dir() -> PathBuf {
    std::env::temp_dir().join(format!("russet-runtime-{}", fsutil::current_uid().unwrap_or(0)))
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            runtime_dir: fallback_runtime_dir(),
            state_dir: None,
            broker: broker::BrokerOptions::default(),
            broker_transport: if cfg!(unix) { BrokerTransport::Unix } else { BrokerTransport::Tcp },
            log_capacity: logs::CAPACITY,
            lease_grace: Duration::from_secs(2),
            keep_ended: 50,
        }
    }
}

/// `run.start` without a debugger (RD §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    pub device: DeviceSelector,
    /// The bundle ID on the device (after the bundle-ID policy).
    pub bundle_id: String,
    pub args: Vec<String>,
    /// The user's environment; Russet's defaults are added (RD §5.3).
    pub env: BTreeMap<String, String>,
    pub launch: LaunchStrategy,
    pub lease: SessionLease,
    /// The absolute project root, for `SessionSummary.project`.
    pub project: Option<PathBuf>,
    /// The process name for log entries (`CFBundleExecutable`).
    pub process_name: Option<String>,
    /// The local build of the main executable, whose `LC_UUID` `session.json` records.
    pub local_executable: Option<PathBuf>,
}

impl RunRequest {
    pub fn new(device: DeviceSelector, bundle_id: impl Into<String>) -> Self {
        Self {
            device,
            bundle_id: bundle_id.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            launch: LaunchStrategy::Auto,
            lease: SessionLease::Owned,
            project: None,
            process_name: None,
            local_executable: None,
        }
    }
}

/// Which debugger front end will connect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Frontend {
    /// `lldb-dap` behind the DAP hub (W11).
    Dap,
    /// `russet debug` spawning `lldb -s attach.lldb`.
    #[default]
    LldbCli,
    /// No front end: `russet debug prepare` prints the plan.
    None,
}

/// The local build the debugger loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugTarget {
    /// The executable with debug info (unsigned product or signed copy; same UUID).
    pub local_executable: PathBuf,
    pub local_app_dir: Option<PathBuf>,
    /// The SDK, for `target.sdk-path`.
    pub sdk_path: Option<PathBuf>,
    /// `--arch` (`arm64-apple-ios` for devices; `None` reads the file).
    pub arch: Option<String>,
}

/// `debug.prepare`: a suspended launch plus the broker attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugRequest {
    pub run: RunRequest,
    pub stop_on_entry: bool,
    pub frontend: Frontend,
    pub attach_mode: AttachMode,
    /// `swift` (default) or `llvm` (ARCHITECTURE §16.1), for `session.json`.
    pub flavor: russet_protocol::debug::DebuggerFlavor,
    /// The resolved LLDB and lldb-dap (`ToolInfo.env` is applied to every LLDB child, ARCHITECTURE §16.2).
    pub lldb: Option<russet_toolchain::ToolInfo>,
    pub lldb_dap: Option<russet_toolchain::ToolInfo>,
    pub target: DebugTarget,
    pub platform: LldbPlatform,
    pub init: InitSettings,
    /// `attach.lldb` goes to `<script_root>/<sid>/attach.lldb`; the pipeline passes `<project>/.russet/debug`
    /// (ARCHITECTURE §14.8). `None`: `state/sessions/<sid>/`.
    pub script_root: Option<PathBuf>,
}

/// What `debug.attach` attaches to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachTarget {
    /// A running process id.
    Pid(u32),
    /// The newest process of a bundle id (resolved by the device plane).
    Bundle(String),
}

/// `debug.attach` to a running process (RD §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachRequest {
    pub device: DeviceSelector,
    pub target: AttachTarget,
    pub frontend: Frontend,
    pub lease: SessionLease,
    /// The local build with debug info to load (its UUID must match the running image); optional.
    pub debug_target: Option<DebugTarget>,
    /// The resolved LLDB, for `PreparedDebug.lldb`.
    pub lldb: Option<russet_toolchain::ToolInfo>,
    pub platform: LldbPlatform,
    pub init: InitSettings,
    pub script_root: Option<PathBuf>,
}

/// Where LLDB connects to the broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerEndpoint {
    Unix { path: PathBuf },
    Tcp { addr: SocketAddr },
}

/// The per-session DAP hub endpoint (ARCHITECTURE §10.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DapEndpoint {
    Unix { path: PathBuf },
    NamedPipe { name: String },
}

/// How to start LLDB: program, args, env (the resolver's env contract), init and attach commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LldbLaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub init_commands: Vec<String>,
    pub attach_commands: Vec<String>,
}

/// Session states (the domain view); [`SessionState::wire`] gives the kebab-case `SessionState` of
/// ARCHITECTURE §9.5. Whether a debugger is attached is `SessionSummary.debugger`, not a state [D 84].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Preparing,
    Building,
    Installing,
    Launching,
    Attaching,
    Arming,
    Ready,
    Running,
    Stopped,
    Exited { code: i32 },
    Crashed { crash_id: Option<String> },
    Lost,
    Failed,
}

/// A session event. W11 and W18 add more variants (JIT stats, crashes, warnings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    State(SessionState),
    Debugger(DebuggerState),
    /// The session ended (`run.exited`), with the error when Russet knows why it failed.
    Ended {
        state: WireSessionState,
        exit: Option<SessionExit>,
        error: Option<russet_protocol::run::SessionError>,
    },
}

/// How to end a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateMode {
    Kill,
    Detach,
}

/// `run.hotReload`'s result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotReloadResult {
    pub supported: bool,
    pub kind: String,
    pub ms: u64,
    pub message: Option<String>,
}

/// A prepared debug session: the app is launched suspended and the broker holds the attach.
#[derive(Debug, Clone)]
pub struct PreparedDebug {
    pub session: SessionHandle,
    pub pid: u32,
    pub broker: BrokerEndpoint,
    pub lldb: LldbLaunchPlan,
    pub dap: Option<DapEndpoint>,
    /// The URL in `process connect`.
    pub lldb_url: String,
    /// The `attach.lldb` the daemon wrote.
    pub attach_script: Option<PathBuf>,
    pub attach_mode: AttachMode,
    /// `attachCommands` for `lldb-dap` (`!`-prefixed, RD §9.2).
    pub dap_attach_commands: Vec<String>,
}

/// How a session relaunches its app (Swift Quick Relaunch, RD §4.7): set by [`DebugService`], invoked by
/// [`SessionHandle::relaunch`] and the DAP hub's `russet/relaunch`.
#[async_trait]
pub trait Relaunch: Send + Sync {
    /// Relaunches `session`'s app in place (same session id) and returns the new pid.
    async fn relaunch(&self, session: &SessionHandle) -> Result<u32>;
}

/// One agent breakpoint the daemon tracks so it can merge per source and remove by id (ARCHITECTURE §10.2).
#[derive(Debug, Clone)]
struct AgentBreakpoint {
    session: String,
    file: String,
    line: u32,
    condition: Option<String>,
    log_message: Option<String>,
}

/// The daemon's agent-breakpoint registry (headless merge per source).
#[derive(Debug, Default)]
struct BreakpointStore {
    next_id: u32,
    items: BTreeMap<u32, AgentBreakpoint>,
}

/// The session registry (`DaemonCtx.debug`).
pub struct DebugService {
    dev: Arc<dyn DeviceServices>,
    cfg: DebugConfig,
    sessions: Mutex<BTreeMap<String, SessionHandle>>,
    created: broadcast::Sender<SessionHandle>,
    breakpoints: Mutex<BreakpointStore>,
}

impl std::fmt::Debug for DebugService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugService").field("cfg", &self.cfg).field("sessions", &lock(&self.sessions).len()).finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl DebugService {
    pub fn new(dev: Arc<dyn DeviceServices>, cfg: DebugConfig) -> Self {
        let (created, _) = broadcast::channel(64);
        Self {
            dev,
            cfg,
            sessions: Mutex::new(BTreeMap::new()),
            created,
            breakpoints: Mutex::new(BreakpointStore::default()),
        }
    }

    /// Sessions as they are created (`run.subscribe` without a session id).
    pub fn watch_new(&self) -> broadcast::Receiver<SessionHandle> {
        self.created.subscribe()
    }

    pub fn config(&self) -> &DebugConfig {
        &self.cfg
    }

    /// The device plane this service uses.
    pub fn devices(&self) -> Arc<dyn DeviceServices> {
        Arc::clone(&self.dev)
    }

    /// Launches without a debugger.
    pub async fn run(&self, r: RunRequest) -> Result<SessionHandle> {
        let facts = SessionFacts {
            app_executable: r.process_name.clone(),
            app_uuid: local_uuid(r.local_executable.clone()).await,
            ..SessionFacts::default()
        };
        let h = self.new_session(SessionKind::Run, &r, None, facts);
        match self.run_inner(&h, r).await {
            Ok(()) => Ok(h),
            Err(e) => {
                h.end(SessionState::Failed, None);
                h.abort().await;
                Err(e)
            }
        }
    }

    async fn run_inner(&self, h: &SessionHandle, r: RunRequest) -> Result<()> {
        let udid = self.dev.resolve(&r.device).await?;
        h.set_udid(&udid.0);
        self.describe_device(h, &udid).await;
        h.set_state(SessionState::Launching);
        let mut spec = LaunchSpec::new(r.bundle_id.clone(), r.args.clone(), &r.env);
        spec.strategy = r.launch;
        let mut launched = self.dev.launch(&udid, &spec).await?;
        h.set_pid(launched.pid);
        record_launch(h, &launched);
        let process = ProcessRef { dev: self.devices(), udid: udid.clone(), pid: launched.pid };
        if let Err(control) = h.set_control(Control::Process(process)).await {
            session::stop_control(control).await;
            return Err(stopped_while_launching());
        }
        let stdio_done = self.pump_output(h, &udid, &mut launched, r.process_name.clone()).await;
        if let Some(channel) = launched.channel.take() {
            h.keep_alive(Box::new(channel));
        }
        h.set_state(SessionState::Running);
        watch_exit(h.clone(), h.generation(), launched.exit.take(), stdio_done);
        h.set_relauncher(Arc::new(RunRelauncher {
            dev: self.devices(),
            udid,
            spec,
            process_name: r.process_name.clone(),
        }));
        Ok(())
    }

    /// Suspended launch plus the broker attach (or a plain forward for `legacy-async`).
    pub async fn prepare_debug(&self, r: DebugRequest) -> Result<PreparedDebug> {
        let facts = SessionFacts {
            app_executable: r.run.process_name.clone(),
            app_uuid: local_uuid(Some(r.target.local_executable.clone())).await,
            lldb: r.lldb.as_ref().map(|t| lldb_facts(r.flavor, t)),
            ..SessionFacts::default()
        };
        let h = self.new_session(SessionKind::Debug, &r.run, Some(r.attach_mode), facts);
        match self.prepare_inner(&h, r).await {
            Ok(p) => Ok(p),
            Err(e) => {
                h.end(SessionState::Failed, None);
                // Whatever already runs (the broker, the suspended app) must not outlive the failure.
                h.abort().await;
                Err(e)
            }
        }
    }

    async fn prepare_inner(&self, h: &SessionHandle, r: DebugRequest) -> Result<PreparedDebug> {
        let udid = self.dev.resolve(&r.run.device).await?;
        h.set_udid(&udid.0);
        self.describe_device(h, &udid).await;
        h.set_state(SessionState::Launching);
        let mut spec = LaunchSpec::new(r.run.bundle_id.clone(), r.run.args.clone(), &r.run.env);
        spec.strategy = r.run.launch;
        spec.suspended = true;
        let mut launched = self.dev.launch(&udid, &spec).await?;
        let pid = launched.pid;
        h.set_pid(pid);
        record_launch(h, &launched);
        // Anything that fails from here on must not leave a stopped app behind.
        let cleanup = |dev: Arc<dyn DeviceServices>, udid: Udid| async move {
            let _ = dev.signal(&udid, pid, 9).await;
        };
        // A stop that arrived during the launch: no attach for an app that is about to be killed. (A later one is
        // caught when the broker becomes the session's control.)
        if h.is_ended() {
            cleanup(self.devices(), udid.clone()).await;
            return Err(stopped_while_launching());
        }
        let stdio_done = self.pump_output(h, &udid, &mut launched, r.run.process_name.clone()).await;
        h.set_state(SessionState::Attaching);
        let stream = match self.dev.connect_debugserver(&udid).await {
            Ok(s) => s,
            Err(e) => {
                cleanup(self.devices(), udid.clone()).await;
                return Err(
                    errors::debugserver_unavailable(e.message()).with_details(e.to_data().details.unwrap_or_default())
                );
            }
        };
        let spec_ep = self.endpoint_spec(h.id());
        #[cfg(unix)]
        if let EndpointSpec::Unix { path } = &spec_ep
            && let Some(dir) = path.parent()
            && dir == self.cfg.runtime_dir.join("sessions").join(h.id().as_str())
        {
            h.keep_alive(Box::new(RuntimeDirGuard(dir.to_path_buf())));
        }
        let (endpoint, lldb_url) = match r.attach_mode {
            AttachMode::Broker => {
                let opts = self.cfg.broker.clone();
                let opts_handoff = opts.handoff_timeout;
                let mut b = match Broker::connect(stream, opts).await {
                    Ok(b) => b,
                    Err(e) => {
                        cleanup(self.devices(), udid.clone()).await;
                        return Err(e);
                    }
                };
                if let Err(e) = b.attach(pid).await {
                    cleanup(self.devices(), udid.clone()).await;
                    return Err(e);
                }
                // vAttach done: the launch channel may close now (RD §4.2 step 10).
                drop(launched.channel.take());
                let handle = match b.serve(spec_ep).await {
                    Ok(handle) => handle,
                    Err(e) => {
                        cleanup(self.devices(), udid.clone()).await;
                        return Err(e);
                    }
                };
                let endpoint = handle.endpoint().clone();
                let url = handle.lldb_url();
                let ends = BrokerEnds { dev: self.devices(), udid: udid.clone(), pid, handoff: opts_handoff };
                follow_broker(h.clone(), h.generation(), handle.subscribe(), ends);
                let process = ProcessRef { dev: self.devices(), udid: udid.clone(), pid };
                if let Err(control) = h.set_control(Control::Broker { handle, process }).await {
                    session::stop_control(control).await;
                    return Err(stopped_while_launching());
                }
                (endpoint, url)
            }
            AttachMode::LegacyAsync => {
                let fwd = match broker::serve_plain_forward(stream, spec_ep, self.cfg.broker.handoff_timeout).await {
                    Ok(f) => f,
                    Err(e) => {
                        cleanup(self.devices(), udid.clone()).await;
                        return Err(e);
                    }
                };
                let endpoint = fwd.endpoint().clone();
                let url = broker::lldb_url(&endpoint);
                // LLDB attaches itself; keep the launch channel open for the whole session.
                if let Some(channel) = launched.channel.take() {
                    h.keep_alive(Box::new(channel));
                }
                h.keep_alive(Box::new(fwd));
                let process = ProcessRef { dev: self.devices(), udid: udid.clone(), pid };
                if let Err(control) = h.set_control(Control::Process(process)).await {
                    session::stop_control(control).await;
                    return Err(stopped_while_launching());
                }
                (endpoint, url)
            }
        };
        match r.attach_mode {
            // LLDB drives the process: the device's exit notice (or the end of stdout) ends the session.
            AttachMode::LegacyAsync => watch_exit(h.clone(), h.generation(), launched.exit.take(), stdio_done),
            // The broker reports exits while it holds the process; the device's notice takes over after a detach.
            AttachMode::Broker => h.stash_exit_signals(ExitSignals { exit: launched.exit.take(), stdio_done }),
        }

        let plan = AttachPlan {
            platform: r.platform.clone(),
            arch: r.target.arch.clone(),
            local_executable: r.target.local_executable.clone(),
            device_app_dir: launched.device_app_dir(),
            local_app_dir: r.target.local_app_dir.clone(),
            broker_url: lldb_url.clone(),
        };
        let mut init_settings = r.init.clone();
        if init_settings.sdk_path.is_none() {
            init_settings.sdk_path.clone_from(&r.target.sdk_path);
        }
        let init = lldb::init_commands(&init_settings);
        let attach = match r.attach_mode {
            AttachMode::Broker => lldb::attach_commands(&plan),
            AttachMode::LegacyAsync => lldb::legacy_async_commands(&plan, pid, launched.executable.as_deref()),
        };
        let script_dir = match &r.script_root {
            Some(root) => Some(root.join(h.id().as_str())),
            None => self.cfg.state_dir.as_ref().map(|d| d.join("sessions").join(h.id().as_str())),
        };
        let attach_script = match script_dir {
            Some(dir) => {
                let path = dir.join("attach.lldb");
                let mut lines = init.clone();
                lines.extend(attach.iter().cloned());
                fsutil::private_dir(&dir).map_err(|e| errors::io("creating the debug script directory", e))?;
                fsutil::atomic_write(&path, lldb::script_text(&lines).as_bytes())
                    .map_err(|e| errors::io("writing attach.lldb", e))?;
                Some(path)
            }
            None => None,
        };
        let (program, env) = match &r.lldb {
            Some(t) => (t.path.clone(), t.env.clone()),
            None => (PathBuf::from("lldb"), BTreeMap::new()),
        };
        let args = attach_script.as_deref().map(lldb::interactive_args).unwrap_or_default();
        h.update_facts(|f| f.lldb_url = Some(lldb_url.clone()));
        let dap_attach_commands = lldb::dap_attach_commands(&plan);
        // Quick Relaunch (RD §4.7): relaunch suspended and re-attach the broker on the same endpoint.
        if r.attach_mode == AttachMode::Broker {
            h.set_relauncher(Arc::new(DebugRelauncher {
                dev: self.devices(),
                udid: udid.clone(),
                spec: spec.clone(),
                endpoint: self.endpoint_spec(h.id()),
                broker: self.cfg.broker.clone(),
                handoff: self.cfg.broker.handoff_timeout,
            }));
        }
        // DAP hub (frontend: dap): the daemon owns lldb-dap; the IDE and agents share it (ARCHITECTURE §10.2).
        let dap = if r.frontend == Frontend::Dap {
            self.start_dap_hub(h, r.lldb_dap.as_ref(), &init, &dap_attach_commands).await?
        } else {
            None
        };
        h.set_state(SessionState::Ready);
        Ok(PreparedDebug {
            session: h.clone(),
            pid,
            broker: endpoint,
            lldb: LldbLaunchPlan { program, args, env, init_commands: init, attach_commands: attach },
            dap,
            lldb_url,
            attach_script,
            attach_mode: r.attach_mode,
            dap_attach_commands,
        })
    }

    /// Starts the per-session DAP hub over a daemon-owned `lldb-dap`, drives a headless `initialize` + `attach`
    /// so agents can debug without an IDE, serves the IDE socket, and stores the hub on the session. Returns the
    /// hub endpoint. `lldb_dap` must be a resolved, `ok` tool.
    async fn start_dap_hub(
        &self,
        h: &SessionHandle,
        lldb_dap: Option<&russet_toolchain::ToolInfo>,
        init: &[String],
        dap_attach_commands: &[String],
    ) -> Result<Option<DapEndpoint>> {
        let tool = lldb_dap.filter(|t| t.ok).ok_or_else(|| {
            errors::error(
                codes::debug::LLDB_UNSUPPORTED,
                "lldb-dap is required for the DAP front end but is unavailable",
            )
        })?;
        let backend = dap::spawn_lldb_dap(&tool.path, &[], &tool.env)?;
        let hooks: Arc<dyn dap::HubHooks> = Arc::new(SessionHooks { session: h.clone() });
        let hub = Arc::new(dap::DapHub::start(backend, hooks));
        let config = serde_json::json!({
            "initCommands": init,
            "attachCommands": dap_attach_commands,
            "stopOnEntry": false,
        });
        hub.bootstrap_attach(config).await?;
        let endpoint = self.dap_endpoint(h.id());
        #[cfg(unix)]
        hub.serve_ide(&endpoint).await?;
        h.set_dap_hub(Arc::clone(&hub));
        h.keep_alive(Box::new(hub));
        Ok(Some(endpoint))
    }

    /// The DAP hub endpoint for a session (`runtime/sessions/<sid>/dap.sock`, ARCHITECTURE §10.2).
    fn dap_endpoint(&self, sid: &SessionId) -> DapEndpoint {
        #[cfg(unix)]
        {
            DapEndpoint::Unix { path: self.cfg.runtime_dir.join("sessions").join(sid.as_str()).join("dap.sock") }
        }
        #[cfg(not(unix))]
        {
            DapEndpoint::NamedPipe { name: format!("russet-dap-{}", sid.as_str()) }
        }
    }

    /// `debug.attach`: attach the broker to an already-running process (RD §4.3). The app keeps running if the
    /// broker's own connection drops (`QSetDetachOnError:1`).
    pub async fn attach(&self, r: AttachRequest) -> Result<PreparedDebug> {
        let udid = self.dev.resolve(&r.device).await?;
        let pid = match &r.target {
            AttachTarget::Pid(pid) => *pid,
            AttachTarget::Bundle(b) => {
                return Err(Error::new(
                    codes::core::INVALID_ARGUMENT,
                    format!("attaching by bundle id (`{b}`) needs the device process list"),
                )
                .with_hint("Pass the pid (`--pid N`); `russet process list` shows running processes."));
            }
        };
        let bundle_id = match &r.target {
            AttachTarget::Bundle(b) => b.clone(),
            AttachTarget::Pid(_) => String::new(),
        };
        let mut run = RunRequest::new(r.device.clone(), bundle_id);
        run.lease = r.lease;
        let facts = SessionFacts {
            app_uuid: match &r.debug_target {
                Some(t) => local_uuid(Some(t.local_executable.clone())).await,
                None => None,
            },
            lldb: r.lldb.as_ref().map(|t| lldb_facts(russet_protocol::debug::DebuggerFlavor::Swift, t)),
            ..SessionFacts::default()
        };
        let h = self.new_session(SessionKind::Debug, &run, Some(AttachMode::Broker), facts);
        h.set_udid(&udid.0);
        h.set_pid(pid);
        self.describe_device(&h, &udid).await;
        match self.attach_inner(&h, &udid, pid, r).await {
            Ok(p) => Ok(p),
            Err(e) => {
                h.end(SessionState::Failed, None);
                h.abort().await;
                Err(e)
            }
        }
    }

    async fn attach_inner(&self, h: &SessionHandle, udid: &Udid, pid: u32, r: AttachRequest) -> Result<PreparedDebug> {
        h.set_state(SessionState::Attaching);
        let stream =
            self.dev.connect_debugserver(udid).await.map_err(|e| errors::debugserver_unavailable(e.message()))?;
        let spec_ep = self.endpoint_spec(h.id());
        #[cfg(unix)]
        if let EndpointSpec::Unix { path } = &spec_ep
            && let Some(dir) = path.parent()
            && dir == self.cfg.runtime_dir.join("sessions").join(h.id().as_str())
        {
            h.keep_alive(Box::new(RuntimeDirGuard(dir.to_path_buf())));
        }
        let mut opts = self.cfg.broker.clone();
        opts.detach_on_error = true; // attach-to-running: keep the app alive if the broker drops.
        let opts_handoff = opts.handoff_timeout;
        let mut broker = Broker::connect(stream, opts).await?;
        broker.attach(pid).await?;
        let handle = broker.serve(spec_ep).await?;
        let endpoint = handle.endpoint().clone();
        let lldb_url = handle.lldb_url();
        let ends = BrokerEnds { dev: self.devices(), udid: udid.clone(), pid, handoff: opts_handoff };
        follow_broker(h.clone(), h.generation(), handle.subscribe(), ends);
        let process = ProcessRef { dev: self.devices(), udid: udid.clone(), pid };
        if let Err(control) = h.set_control(Control::Broker { handle, process }).await {
            session::stop_control(control).await;
            return Err(stopped_while_launching());
        }
        // Logs by pid, when the device plane can stream os_trace.
        if let Ok(Some(mut rx)) = self.dev.os_log(udid, pid).await {
            let view = h.logs();
            let session = h.clone();
            tokio::spawn(async move {
                while let Some(e) = rx.recv().await {
                    if session.is_ended() {
                        break;
                    }
                    view.push(e);
                }
            });
        }
        let plan = AttachPlan {
            platform: r.platform.clone(),
            arch: r.debug_target.as_ref().and_then(|t| t.arch.clone()),
            local_executable: r.debug_target.as_ref().map(|t| t.local_executable.clone()).unwrap_or_default(),
            device_app_dir: None,
            local_app_dir: r.debug_target.as_ref().and_then(|t| t.local_app_dir.clone()),
            broker_url: lldb_url.clone(),
        };
        let mut init_settings = r.init.clone();
        if init_settings.sdk_path.is_none() {
            init_settings.sdk_path = r.debug_target.as_ref().and_then(|t| t.sdk_path.clone());
        }
        let init = lldb::init_commands(&init_settings);
        let attach = lldb::attach_commands(&plan);
        let attach_script = self.write_attach_script(h, &r.script_root, &init, &attach)?;
        let (program, env) =
            r.lldb.as_ref().map_or((PathBuf::from("lldb"), BTreeMap::new()), |t| (t.path.clone(), t.env.clone()));
        let args = attach_script.as_deref().map(lldb::interactive_args).unwrap_or_default();
        h.update_facts(|f| f.lldb_url = Some(lldb_url.clone()));
        h.set_state(SessionState::Ready);
        Ok(PreparedDebug {
            session: h.clone(),
            pid,
            broker: endpoint,
            lldb: LldbLaunchPlan { program, args, env, init_commands: init, attach_commands: attach.clone() },
            dap: None,
            lldb_url,
            attach_script,
            attach_mode: AttachMode::Broker,
            dap_attach_commands: lldb::dap_attach_commands(&plan),
        })
    }

    /// Writes `attach.lldb` (init + attach commands) under the session's script root, when one is set.
    fn write_attach_script(
        &self,
        h: &SessionHandle,
        script_root: &Option<PathBuf>,
        init: &[String],
        attach: &[String],
    ) -> Result<Option<PathBuf>> {
        let dir = match script_root {
            Some(root) => Some(root.join(h.id().as_str())),
            None => self.cfg.state_dir.as_ref().map(|d| d.join("sessions").join(h.id().as_str())),
        };
        match dir {
            Some(dir) => {
                let path = dir.join("attach.lldb");
                let mut lines = init.to_vec();
                lines.extend(attach.iter().cloned());
                fsutil::private_dir(&dir).map_err(|e| errors::io("creating the debug script directory", e))?;
                fsutil::atomic_write(&path, lldb::script_text(&lines).as_bytes())
                    .map_err(|e| errors::io("writing attach.lldb", e))?;
                Ok(Some(path))
            }
            None => Ok(None),
        }
    }

    /// `debug.control`: drive execution through the session's DAP hub (agent role).
    pub async fn control(
        &self,
        session_id: &str,
        action: russet_protocol::debug::ControlAction,
        thread_id: Option<u64>,
        wait_for_stop_ms: Option<u64>,
    ) -> Result<russet_protocol::debug::DebugControlResult> {
        let mut client = self.agent_client(session_id)?;
        dap::agent::control(&mut client, action, thread_id, wait_for_stop_ms).await
    }

    /// `debug.inspect`.
    pub async fn inspect(
        &self,
        session_id: &str,
        what: russet_protocol::debug::InspectWhat,
        thread_id: Option<u64>,
        frame_id: Option<u64>,
        scope: Option<&str>,
        depth: Option<u32>,
    ) -> Result<russet_protocol::debug::DebugInspectResult> {
        use russet_protocol::debug::InspectWhat;
        let client = self.agent_client(session_id)?;
        match what {
            InspectWhat::Threads => dap::agent::threads(&client).await,
            InspectWhat::Stack => dap::agent::stack(&client, thread_id, depth).await,
            InspectWhat::Variables => dap::agent::variables(&client, frame_id, scope).await,
        }
    }

    /// `debug.evaluate`.
    pub async fn evaluate(
        &self,
        session_id: &str,
        expression: &str,
        frame_id: Option<u64>,
        context: &str,
    ) -> Result<russet_protocol::debug::DebugEvaluateResult> {
        let client = self.agent_client(session_id)?;
        dap::agent::evaluate(&client, expression, frame_id, context).await
    }

    /// `debug.breakpoint.set`: record the agent breakpoint and push the merged set for its source to the hub.
    pub async fn breakpoint_set(
        &self,
        session_id: &str,
        file: &str,
        line: u32,
        condition: Option<String>,
        log_message: Option<String>,
    ) -> Result<russet_protocol::debug::DebugBreakpointSetResult> {
        let id = {
            let mut store = lock(&self.breakpoints);
            store.next_id += 1;
            let id = store.next_id;
            store.items.insert(
                id,
                AgentBreakpoint { session: session_id.to_owned(), file: file.to_owned(), line, condition, log_message },
            );
            id
        };
        let verified = self.push_breakpoints(session_id, file).await.unwrap_or(false);
        Ok(russet_protocol::debug::DebugBreakpointSetResult { breakpoint_id: id, verified, pending: !verified })
    }

    /// `debug.breakpoint.remove`: drop the breakpoint and re-push its source's set.
    pub async fn breakpoint_remove(&self, breakpoint_id: u32) -> Result<()> {
        let removed = lock(&self.breakpoints).items.remove(&breakpoint_id);
        if let Some(bp) = removed {
            let _ = self.push_breakpoints(&bp.session, &bp.file).await;
        }
        Ok(())
    }

    /// Pushes `setBreakpoints` for one source with every current agent breakpoint in it. Returns whether the
    /// debugger verified them. A session without a DAP hub keeps the breakpoint pending (returned as unverified).
    async fn push_breakpoints(&self, session_id: &str, file: &str) -> Result<bool> {
        let lines: Vec<serde_json::Value> = {
            let store = lock(&self.breakpoints);
            store
                .items
                .values()
                .filter(|b| b.session == session_id && b.file == file)
                .map(|b| {
                    let mut o = serde_json::json!({ "line": b.line });
                    if let Some(c) = &b.condition {
                        o["condition"] = serde_json::json!(c);
                    }
                    if let Some(m) = &b.log_message {
                        o["logMessage"] = serde_json::json!(m);
                    }
                    o
                })
                .collect()
        };
        let Some(client) = self.agent_client(session_id).ok() else { return Ok(false) };
        let args = serde_json::json!({ "source": { "path": file }, "breakpoints": lines });
        let body = client.request("setBreakpoints", args).await?;
        Ok(body.get("breakpoints").and_then(|b| b.as_array()).is_some_and(|arr| {
            arr.iter().all(|b| b.get("verified").and_then(serde_json::Value::as_bool) == Some(true))
        }))
    }

    /// An agent DAP client for a session, or `debug.sessionNotFound` / a "no DAP hub" error.
    fn agent_client(&self, session_id: &str) -> Result<dap::DapClient> {
        let h = self.session_by_str(session_id).ok_or_else(|| errors::session_not_found(session_id))?;
        let hub = h.dap_hub().ok_or_else(|| {
            Error::new(codes::debug::SESSION_NOT_FOUND, "this session has no DAP hub")
                .with_hint("Start it with `debug.prepare {frontend: dap}` (or attach an IDE), then drive it.")
        })?;
        Ok(hub.agent_client())
    }

    /// `run.relaunch` / `run.hotReload` (Swift Quick Relaunch): relaunch the session's app in place.
    pub async fn relaunch(&self, session_id: &str) -> Result<(u32, u64)> {
        let h = self.session_by_str(session_id).ok_or_else(|| errors::session_not_found(session_id))?;
        h.relaunch().await
    }

    pub fn session(&self, id: &SessionId) -> Option<SessionHandle> {
        lock(&self.sessions).get(id.as_str()).cloned()
    }

    /// A session by its wire id.
    pub fn session_by_str(&self, id: &str) -> Option<SessionHandle> {
        lock(&self.sessions).get(id).cloned()
    }

    /// Every session in memory, oldest first (live ones and the most recent ended ones).
    pub fn sessions(&self) -> Vec<SessionSummary> {
        lock(&self.sessions).values().map(SessionHandle::summary).collect()
    }

    /// Every session handle, oldest first.
    pub fn handles(&self) -> Vec<SessionHandle> {
        lock(&self.sessions).values().cloned().collect()
    }

    fn new_session(
        &self,
        kind: SessionKind,
        r: &RunRequest,
        attach_mode: Option<AttachMode>,
        facts: SessionFacts,
    ) -> SessionHandle {
        let h = SessionHandle::create(SessionInit {
            id: new_session_id(),
            kind,
            bundle_id: r.bundle_id.clone(),
            project: r.project.as_ref().map(|p| p.to_string_lossy().into_owned()),
            lease: r.lease,
            attach_mode,
            state_dir: self.cfg.state_dir.clone(),
            log_capacity: self.cfg.log_capacity,
            facts,
        });
        let _ = self.created.send(h.clone());
        let mut map = lock(&self.sessions);
        map.insert(h.id().as_str().to_owned(), h.clone());
        // Forget the oldest ended sessions beyond `keep_ended`.
        let ended: Vec<String> = map.iter().filter(|(_, s)| s.is_ended()).map(|(k, _)| k.clone()).collect();
        if ended.len() > self.cfg.keep_ended {
            for k in &ended[..ended.len() - self.cfg.keep_ended] {
                map.remove(k);
            }
        }
        h
    }

    /// Records the device's facts for `session.json`; a device plane that cannot describe it is not an error.
    async fn describe_device(&self, h: &SessionHandle, udid: &Udid) {
        match self.dev.describe(udid).await {
            Ok(Some(facts)) => h.update_facts(|f| f.device = facts),
            Ok(None) => {}
            Err(e) => tracing::debug!(error = %e, "no device facts for session.json"),
        }
    }

    fn endpoint_spec(&self, sid: &SessionId) -> EndpointSpec {
        match self.cfg.broker_transport {
            #[cfg(unix)]
            BrokerTransport::Unix => {
                EndpointSpec::Unix { path: broker::socket_path(&self.cfg.runtime_dir, sid.as_str(), "gdb.sock") }
            }
            _ => EndpointSpec::Tcp { bind: SocketAddr::from(([127, 0, 0, 1], 0)) },
        }
    }

    /// Starts the stdout pump and the os_trace pump; returns a receiver that fires when stdout ends.
    async fn pump_output(
        &self,
        h: &SessionHandle,
        udid: &Udid,
        launched: &mut Launched,
        process: Option<String>,
    ) -> Option<tokio::sync::oneshot::Receiver<()>> {
        pump_output(&self.dev, h, udid, launched, process).await
    }
}

/// Starts the stdout and os_trace pumps for a launch; returns a receiver that fires when stdout ends. Shared by
/// the initial launch and Quick Relaunch.
async fn pump_output(
    dev: &Arc<dyn DeviceServices>,
    h: &SessionHandle,
    udid: &Udid,
    launched: &mut Launched,
    process: Option<String>,
) -> Option<tokio::sync::oneshot::Receiver<()>> {
    let pid = launched.pid;
    let mut done = launched.stdio.take().map(|stdio| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let view = h.logs();
        let process = process.clone();
        tokio::spawn(async move {
            logs::pump_stdio(view, stdio, Some(pid), process).await;
            let _ = tx.send(());
        });
        rx
    });
    if let Some(mut entries) = launched.entries.take() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let view = h.logs();
        tokio::spawn(async move {
            while let Some(e) = entries.recv().await {
                view.push(e);
            }
            let _ = tx.send(());
        });
        done = done.or(Some(rx));
    }
    match dev.os_log(udid, pid).await {
        Ok(Some(mut rx)) => {
            let view = h.logs();
            let session = h.clone();
            tokio::spawn(async move {
                while let Some(e) = rx.recv().await {
                    if session.is_ended() {
                        break;
                    }
                    view.push(e);
                }
            });
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "os_trace for the session is unavailable"),
    }
    done
}

/// Removes a session's own runtime directory (`runtime/sessions/<sid>/`, home of the broker socket) when the
/// session ends; a socket that fell back to a shorter path elsewhere is the listener's to remove.
#[cfg(unix)]
struct RuntimeDirGuard(PathBuf);

#[cfg(unix)]
impl Drop for RuntimeDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("gdb.sock"));
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// The local executable's `LC_UUID` (read off the async threads); `None` when unknown.
async fn local_uuid(path: Option<PathBuf>) -> Option<String> {
    let path = path?;
    tokio::task::spawn_blocking(move || macho::identity(&path).ok().flatten().and_then(|id| id.uuid)).await.ok()?
}

/// `session.json`'s `lldb`: the flavor, the version and the libpython (`PYTHONHOME`'s last component).
fn lldb_facts(flavor: russet_protocol::debug::DebuggerFlavor, t: &russet_toolchain::ToolInfo) -> LldbFacts {
    let python = t.env.get("PYTHONHOME").and_then(|home| {
        Path::new(home).file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty())
    });
    let flavor = match flavor {
        russet_protocol::debug::DebuggerFlavor::Swift => "swift",
        russet_protocol::debug::DebuggerFlavor::Llvm => "llvm",
    };
    LldbFacts { flavor: flavor.to_owned(), version: t.version.clone(), python }
}

/// What the launch told `session.json`: the strategy, the app's directory and executable on the device.
fn record_launch(h: &SessionHandle, launched: &Launched) {
    let strategy = match launched.strategy {
        LaunchStrategy::Auto => "auto",
        LaunchStrategy::CoreDevice => "coredevice",
        LaunchStrategy::Dvt => "dvt",
    };
    let dir = launched.device_app_dir();
    let exe = launched.executable.as_deref().and_then(|e| e.rsplit('/').next()).map(str::to_owned);
    h.update_facts(|f| {
        f.launch = Some(strategy.to_owned());
        f.app_device_path = dir.or(f.app_device_path.take());
        if f.app_executable.is_none() {
            f.app_executable = exe;
        }
    });
}

/// A stop (`run.stop`, the owner leaving) arrived while the session was still launching.
fn stopped_while_launching() -> Error {
    Error::new(codes::core::CANCELLED, "the session was stopped while it launched")
}

/// Ends the session when the device reports the exit, or (without an exit notice) when stdout ends. `launch_gen` is the
/// launch generation this watcher belongs to: once a relaunch bumps the generation, this stale watcher stands
/// down (killing the old process must not end the relaunched session).
fn watch_exit(
    h: SessionHandle,
    launch_gen: u32,
    exit: Option<launch::ExitWatch>,
    stdio_done: Option<tokio::sync::oneshot::Receiver<()>>,
) {
    match (exit, stdio_done) {
        (Some(exit), _) => {
            tokio::spawn(async move {
                let status = exit.await;
                if h.generation() == launch_gen {
                    h.end_exited(status.code, status.signal);
                }
            });
        }
        (None, Some(done)) => {
            tokio::spawn(async move {
                if done.await.is_ok()
                    && h.generation() == launch_gen
                    && !h.is_ended()
                    && h.debugger() != DebuggerState::Attached
                {
                    h.end_exited(None, None);
                }
            });
        }
        (None, None) => {}
    }
}

/// What [`follow_broker`] needs once the broker lets go: the process, and the handoff limit for the error.
struct BrokerEnds {
    dev: Arc<dyn DeviceServices>,
    udid: Udid,
    pid: u32,
    handoff: Option<Duration>,
}

/// Maps broker events onto the session. `launch_gen` is the launch generation (see [`watch_exit`]); a stale broker's
/// terminal events are ignored after a relaunch bumps the generation.
fn follow_broker(h: SessionHandle, launch_gen: u32, mut events: broadcast::Receiver<BrokerEvent>, ends: BrokerEnds) {
    let BrokerEnds { dev, udid, pid, handoff } = ends;
    tokio::spawn(async move {
        loop {
            let e = match events.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            if h.generation() != launch_gen {
                return; // a newer launch owns the session now
            }
            match e {
                BrokerEvent::DebuggerConnected => {
                    h.set_debugger(DebuggerState::Attached);
                    h.set_state(SessionState::Stopped);
                }
                BrokerEvent::Resumed => h.set_state(SessionState::Running),
                BrokerEvent::Stopped { .. } => h.set_state(SessionState::Stopped),
                BrokerEvent::DebuggerDetached | BrokerEvent::DebuggerLost(broker::LostPolicy::Detach) => {
                    // Switch the control first: a stop that sees `debugger: detached` must reach the process.
                    let process = ProcessRef { dev: Arc::clone(&dev), udid: udid.clone(), pid };
                    let _ = h.set_control(Control::Process(process)).await;
                    h.set_debugger(DebuggerState::Detached);
                    h.set_state(SessionState::Running);
                    let signals = h.take_exit_signals();
                    watch_exit(h.clone(), launch_gen, signals.exit, signals.stdio_done);
                    return;
                }
                BrokerEvent::DebuggerLost(broker::LostPolicy::Keep) => {
                    h.set_debugger(DebuggerState::None);
                    h.set_state(SessionState::Ready);
                }
                BrokerEvent::DebuggerLost(broker::LostPolicy::Kill) => {}
                BrokerEvent::ProcessExited { code, signal, requested } => {
                    if requested {
                        h.mark_kill_requested();
                    }
                    h.end_exited(code, signal);
                    return;
                }
                BrokerEvent::Lost => {
                    h.end_failed(SessionState::Lost, &errors::session_lost());
                    return;
                }
                BrokerEvent::HandoffTimeout => {
                    h.end_failed(SessionState::Failed, &errors::handoff_timeout(handoff));
                    return;
                }
            }
        }
    });
}

/// Swift Quick Relaunch for a `run` session: relaunch the bundle and swap the session's process in place.
struct RunRelauncher {
    dev: Arc<dyn DeviceServices>,
    udid: Udid,
    spec: LaunchSpec,
    process_name: Option<String>,
}

#[async_trait]
impl Relaunch for RunRelauncher {
    async fn relaunch(&self, h: &SessionHandle) -> Result<u32> {
        let launch_gen = h.next_generation();
        h.set_state(SessionState::Launching);
        let mut launched = self.dev.launch(&self.udid, &self.spec).await?;
        let pid = launched.pid;
        record_launch(h, &launched);
        let process = ProcessRef { dev: Arc::clone(&self.dev), udid: self.udid.clone(), pid };
        h.swap_process(Control::Process(process), pid, SessionState::Running).await;
        let stdio_done = pump_output(&self.dev, h, &self.udid, &mut launched, self.process_name.clone()).await;
        if let Some(channel) = launched.channel.take() {
            h.keep_alive(Box::new(channel));
        }
        watch_exit(h.clone(), launch_gen, launched.exit.take(), stdio_done);
        Ok(pid)
    }
}

/// Swift Quick Relaunch for a broker `debug` session: relaunch suspended, re-attach the broker on the same
/// endpoint, replay the cached breakpoints to the DAP hub, and swap the session's process in place.
struct DebugRelauncher {
    dev: Arc<dyn DeviceServices>,
    udid: Udid,
    spec: LaunchSpec,
    endpoint: EndpointSpec,
    broker: broker::BrokerOptions,
    handoff: Option<Duration>,
}

#[async_trait]
impl Relaunch for DebugRelauncher {
    async fn relaunch(&self, h: &SessionHandle) -> Result<u32> {
        let launch_gen = h.next_generation();
        h.set_state(SessionState::Launching);
        let mut launched = self.dev.launch(&self.udid, &self.spec).await?;
        let pid = launched.pid;
        record_launch(h, &launched);
        let stream =
            self.dev.connect_debugserver(&self.udid).await.map_err(|e| errors::debugserver_unavailable(e.message()))?;
        let mut broker = Broker::connect(stream, self.broker.clone()).await?;
        broker.attach(pid).await?;
        drop(launched.channel.take());
        let handle = broker.serve(self.endpoint.clone()).await?;
        let ends = BrokerEnds { dev: Arc::clone(&self.dev), udid: self.udid.clone(), pid, handoff: self.handoff };
        follow_broker(h.clone(), launch_gen, handle.subscribe(), ends);
        let process = ProcessRef { dev: Arc::clone(&self.dev), udid: self.udid.clone(), pid };
        h.swap_process(Control::Broker { handle, process }, pid, SessionState::Ready).await;
        let stdio_done = pump_output(&self.dev, h, &self.udid, &mut launched, None).await;
        h.stash_exit_signals(ExitSignals { exit: launched.exit.take(), stdio_done });
        // Replay the cached breakpoints to the hub (RD §9.3).
        if let Some(hub) = h.dap_hub() {
            let cache = hub.cached_breakpoints();
            let client = hub.agent_client();
            for args in cache.sources.into_values() {
                let _ = client.request("setBreakpoints", args).await;
            }
            if let Some(f) = cache.functions {
                let _ = client.request("setFunctionBreakpoints", f).await;
            }
            if let Some(e) = cache.exceptions {
                let _ = client.request("setExceptionBreakpoints", e).await;
            }
        }
        Ok(pid)
    }
}

/// [`dap::HubHooks`] backed by a session: `russet/relaunch` runs Quick Relaunch, `russet/deviceLogs` reads the
/// session's merged output.
struct SessionHooks {
    session: SessionHandle,
}

#[async_trait]
impl dap::HubHooks for SessionHooks {
    async fn relaunch(&self) -> Result<u32> {
        Ok(self.session.relaunch().await?.0)
    }
    fn device_logs(&self, since_ms: Option<i64>) -> Vec<serde_json::Value> {
        let filter = russet_protocol::run::OutputFilter::default();
        let (entries, _) = self.session.logs().query(since_ms, Some(200), &filter);
        entries.iter().filter_map(|e| serde_json::to_value(e).ok()).collect()
    }
}

/// Debug doctor checks: whether a broker socket can be created in the runtime directory.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    vec![Arc::new(BrokerEndpointCheck)]
}

/// `debug.brokerEndpoint`: the runtime directory can hold a 0600 broker socket whose path fits `sun_path`.
struct BrokerEndpointCheck;

#[async_trait]
impl russet_doctor::Check for BrokerEndpointCheck {
    fn id(&self) -> &'static str {
        "debug.brokerEndpoint"
    }
    fn title(&self) -> &'static str {
        "Debug broker endpoint"
    }
    fn category(&self) -> russet_doctor::Category {
        russet_doctor::Category::Platform
    }
    fn requires(&self) -> russet_doctor::Requires {
        russet_doctor::Requires::default()
    }
    async fn run(&self, _cx: &russet_doctor::CheckCx) -> russet_doctor::CheckResult {
        let runtime = match std::env::var_os("RUSSET_HOME") {
            Some(home) => PathBuf::from(home).join("runtime"),
            None => match std::env::var_os("XDG_RUNTIME_DIR") {
                Some(r) => PathBuf::from(r).join("russet"),
                None => fallback_runtime_dir(),
            },
        };
        broker_endpoint_probe(&runtime).await
    }
}

/// Binds and removes a broker socket for a dummy session under `runtime`.
pub async fn broker_endpoint_probe(runtime: &Path) -> russet_doctor::CheckResult {
    use russet_doctor::{CheckResult, Status};
    let sid = new_session_id();
    #[cfg(unix)]
    let spec = EndpointSpec::Unix { path: broker::socket_path(runtime, sid.as_str(), "gdb.sock") };
    #[cfg(not(unix))]
    let spec = EndpointSpec::Tcp { bind: SocketAddr::from(([127, 0, 0, 1], 0)) };
    match broker::probe_bind(&spec).await {
        Ok(url) => CheckResult {
            status: Status::Pass,
            summary: "a broker endpoint can be created".to_owned(),
            detail: Some(url),
            hint: None,
        },
        Err(e) => CheckResult {
            status: Status::Fail,
            summary: format!("cannot create a broker endpoint: {}", e.message()),
            detail: Some(runtime.display().to_string()),
            hint: Some(
                "Make the runtime directory writable (it is created 0700), or set RUSSET_HOME to a shorter path."
                    .to_owned(),
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_broker_endpoint_check_passes_in_a_writable_runtime_dir_and_fails_otherwise() {
        let base = std::env::temp_dir().join(format!("russet-doctor-{}", std::process::id()));
        let ok = broker_endpoint_probe(&base.join("runtime")).await;
        assert_eq!(ok.status, russet_doctor::Status::Pass, "{ok:?}");
        assert!(ok.detail.as_deref().is_some_and(|d| d.starts_with("unix-connect://") || d.starts_with("connect://")));
        // A runtime "directory" that is a file cannot hold the socket.
        std::fs::write(base.join("file"), b"x").unwrap_or_else(|e| panic!("{e}"));
        let bad = broker_endpoint_probe(&base.join("file")).await;
        if cfg!(unix) {
            assert_eq!(bad.status, russet_doctor::Status::Fail, "{bad:?}");
            assert!(bad.hint.is_some());
        }
        let _ = std::fs::remove_dir_all(&base);
        let ids: Vec<&str> = checks().iter().map(|c| c.id()).collect();
        assert_eq!(ids, ["debug.brokerEndpoint"]);
    }
}
