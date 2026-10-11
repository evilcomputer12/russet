// SPDX-License-Identifier: GPL-3.0-or-later
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

use async_trait::async_trait;
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
pub mod session;
pub mod symbols;
pub mod wifi;

pub use russet_protocol::common::{
    AttachMode, DebuggerState, LogEntry, SessionExit, SessionKind, SessionLease, SessionState as WireSessionState,
    SessionSummary,
};
pub use session::{SessionHandle, new_session_id};

use crate::broker::{Broker, BrokerEvent, EndpointSpec};
use crate::launch::{InstalledAppInfo, LaunchChannel, LaunchSpec, LaunchStrategy, Launched};
use crate::lldb::{AttachPlan, InitSettings, LldbPlatform};
use crate::session::{Control, SessionInit};

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
        let process = d
            .launch(LaunchRequest {
                bundle_id: spec.bundle_id.clone(),
                args: spec.args.clone(),
                env: spec.env.clone(),
                suspended: spec.suspended,
                kill_existing: spec.kill_existing,
                console: spec.console,
            })
            .await?;
        let mut launched = Launched::pid_only(process.pid);
        launched.executable = Some(process.executable.clone());
        launched.strategy = spec.strategy;
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

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            runtime_dir: std::env::temp_dir().join("russet-runtime"),
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
    /// The resolved LLDB and lldb-dap (`ToolInfo.env` is applied to every LLDB child, ARCHITECTURE §16.2).
    pub lldb: Option<russet_toolchain::ToolInfo>,
    pub lldb_dap: Option<russet_toolchain::ToolInfo>,
    pub target: DebugTarget,
    pub platform: LldbPlatform,
    pub init: InitSettings,
    /// Where `attach.lldb` goes: `<project>/.russet/debug/<sid>/` (ARCHITECTURE §14.8). `None`:
    /// `state/sessions/<sid>/`.
    pub script_dir: Option<PathBuf>,
}

/// `debug.attach` to a running process. W11 defines the fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachRequest {}

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
    /// The session ended (`run.exited`).
    Ended {
        state: WireSessionState,
        exit: Option<SessionExit>,
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

/// The session registry (`DaemonCtx.debug`).
pub struct DebugService {
    dev: Arc<dyn DeviceServices>,
    cfg: DebugConfig,
    sessions: Mutex<BTreeMap<String, SessionHandle>>,
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
        Self { dev, cfg, sessions: Mutex::new(BTreeMap::new()) }
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
        let h = self.new_session(SessionKind::Run, &r, None);
        match self.run_inner(&h, r).await {
            Ok(()) => Ok(h),
            Err(e) => {
                h.end(SessionState::Failed, None);
                Err(e)
            }
        }
    }

    async fn run_inner(&self, h: &SessionHandle, r: RunRequest) -> Result<()> {
        let udid = self.dev.resolve(&r.device).await?;
        h.set_udid(&udid.0);
        h.set_state(SessionState::Launching);
        let mut spec = LaunchSpec::new(r.bundle_id.clone(), r.args.clone(), &r.env);
        spec.strategy = r.launch;
        let mut launched = self.dev.launch(&udid, &spec).await?;
        h.set_pid(launched.pid);
        h.set_control(Control::Process { dev: self.devices(), udid: udid.clone(), pid: launched.pid }).await;
        let stdio_done = self.pump_output(h, &udid, &mut launched, r.process_name.clone()).await;
        if let Some(channel) = launched.channel.take() {
            h.keep_alive(Box::new(channel));
        }
        h.set_state(SessionState::Running);
        watch_exit(h.clone(), launched.exit.take(), stdio_done);
        Ok(())
    }

    /// Suspended launch plus the broker attach (or a plain forward for `legacy-async`).
    pub async fn prepare_debug(&self, r: DebugRequest) -> Result<PreparedDebug> {
        let h = self.new_session(SessionKind::Debug, &r.run, Some(r.attach_mode));
        match self.prepare_inner(&h, r).await {
            Ok(p) => Ok(p),
            Err(e) => {
                h.end(SessionState::Failed, None);
                Err(e)
            }
        }
    }

    async fn prepare_inner(&self, h: &SessionHandle, r: DebugRequest) -> Result<PreparedDebug> {
        let udid = self.dev.resolve(&r.run.device).await?;
        h.set_udid(&udid.0);
        h.set_state(SessionState::Launching);
        let mut spec = LaunchSpec::new(r.run.bundle_id.clone(), r.run.args.clone(), &r.run.env);
        spec.strategy = r.run.launch;
        spec.suspended = true;
        let mut launched = self.dev.launch(&udid, &spec).await?;
        let pid = launched.pid;
        h.set_pid(pid);
        // Anything that fails from here on must not leave a stopped app behind.
        let cleanup = |dev: Arc<dyn DeviceServices>, udid: Udid| async move {
            let _ = dev.signal(&udid, pid, 9).await;
        };
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
        let (endpoint, lldb_url) = match r.attach_mode {
            AttachMode::Broker => {
                let opts = self.cfg.broker.clone();
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
                follow_broker(h.clone(), handle.subscribe(), self.devices(), udid.clone(), pid);
                h.set_control(Control::Broker(handle)).await;
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
                h.set_control(Control::Process { dev: self.devices(), udid: udid.clone(), pid }).await;
                (endpoint, url)
            }
        };
        match r.attach_mode {
            // LLDB drives the process: the device's exit notice (or the end of stdout) ends the session.
            AttachMode::LegacyAsync => watch_exit(h.clone(), launched.exit.take(), stdio_done),
            // The broker reports exits while it holds the process; the device's notice takes over after a detach.
            AttachMode::Broker => h.stash_exit_watch(launched.exit.take()),
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
        let script_dir = r
            .script_dir
            .clone()
            .or_else(|| self.cfg.state_dir.as_ref().map(|d| d.join("sessions").join(h.id().as_str())));
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
        h.set_state(SessionState::Ready);
        Ok(PreparedDebug {
            session: h.clone(),
            pid,
            broker: endpoint,
            lldb: LldbLaunchPlan { program, args, env, init_commands: init, attach_commands: attach },
            dap: None,
            lldb_url,
            attach_script,
            attach_mode: r.attach_mode,
            dap_attach_commands: lldb::dap_attach_commands(&plan),
        })
    }

    /// `debug.attach` (W11).
    pub async fn attach(&self, _r: AttachRequest) -> Result<PreparedDebug> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "attaching to a running app lands with W11")
            .with_hint("Use `russet debug` to launch and debug the app."))
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

    fn new_session(&self, kind: SessionKind, r: &RunRequest, attach_mode: Option<AttachMode>) -> SessionHandle {
        let h = SessionHandle::create(SessionInit {
            id: new_session_id(),
            kind,
            bundle_id: r.bundle_id.clone(),
            project: r.project.as_ref().map(|p| p.to_string_lossy().into_owned()),
            lease: r.lease,
            attach_mode,
            state_dir: self.cfg.state_dir.clone(),
            log_capacity: self.cfg.log_capacity,
        });
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
        let pid = launched.pid;
        let done = launched.stdio.take().map(|stdio| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let view = h.logs();
            let process = process.clone();
            tokio::spawn(async move {
                logs::pump_stdio(view, stdio, Some(pid), process).await;
                let _ = tx.send(());
            });
            rx
        });
        match self.dev.os_log(udid, pid).await {
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
}

/// Ends the session when the device reports the exit, or (without an exit notice) when stdout ends.
fn watch_exit(
    h: SessionHandle,
    exit: Option<launch::ExitWatch>,
    stdio_done: Option<tokio::sync::oneshot::Receiver<()>>,
) {
    match (exit, stdio_done) {
        (Some(exit), _) => {
            tokio::spawn(async move {
                let status = exit.await;
                h.end_exited(status.code, status.signal);
            });
        }
        (None, Some(done)) => {
            tokio::spawn(async move {
                if done.await.is_ok() && !h.is_ended() && h.debugger() != DebuggerState::Attached {
                    h.end_exited(None, None);
                }
            });
        }
        (None, None) => {}
    }
}

/// Maps broker events onto the session.
fn follow_broker(
    h: SessionHandle,
    mut events: broadcast::Receiver<BrokerEvent>,
    dev: Arc<dyn DeviceServices>,
    udid: Udid,
    pid: u32,
) {
    tokio::spawn(async move {
        loop {
            let e = match events.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            match e {
                BrokerEvent::DebuggerConnected => {
                    h.set_debugger(DebuggerState::Attached);
                    h.set_state(SessionState::Stopped);
                }
                BrokerEvent::Resumed => h.set_state(SessionState::Running),
                BrokerEvent::Stopped { .. } => h.set_state(SessionState::Stopped),
                BrokerEvent::DebuggerDetached | BrokerEvent::DebuggerLost(broker::LostPolicy::Detach) => {
                    h.set_debugger(DebuggerState::Detached);
                    h.set_state(SessionState::Running);
                    h.set_control(Control::Process { dev: Arc::clone(&dev), udid: udid.clone(), pid }).await;
                    watch_exit(h.clone(), h.take_exit_watch(), None);
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
                    h.end(SessionState::Lost, None);
                    return;
                }
                BrokerEvent::HandoffTimeout => {
                    h.end(SessionState::Failed, None);
                    return;
                }
            }
        }
    });
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
                None => std::env::temp_dir().join("russet-runtime"),
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
