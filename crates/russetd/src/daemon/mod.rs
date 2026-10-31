// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Daemon lifecycle (ARCHITECTURE §3.3; core §6.3): the single-instance lock, the discovery file, the
//! connection registry, idle exit, shutdown, crash recovery and the development isolation guards.
//!
//! - [`backend`]—`RUSSET_DEVICE_BACKEND` and the live-backend policy of development builds.
//! - [`lock`]—`runtime/russetd.lock` and the host-wide live-device lock.
//! - [`logging`]—daily files, the reloadable filter, redaction, `daemon.logs.subscribe`.
//! - [`panics`]—`state/panics/<ts>.txt`.
//! - [`run`]—`russet daemon run` and the in-process daemon.
//! - [`settings`]—`config.toml` and `mcp-policy.toml` values, through the config and MCP areas' hooks.
//! - `journald`—the redacted journal output under systemd (Linux).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result};
use russet_core::platform::ExecutableId;
use russet_core::rpc::ClientKind;
use russet_ipc::Peer;
use russet_protocol::daemon::{ConnectionSummary, ShutdownReason};
use russet_protocol::initialize::ServerInfo;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use self::logging::LogControl;

pub mod backend;
#[cfg(target_os = "linux")]
pub mod journald;
pub mod lock;
pub mod logging;
pub mod panics;
pub mod run;
pub mod settings;

/// How long jobs get after `daemon.shuttingDown` before their process trees are killed (ARCHITECTURE §3.3
/// item 5).
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// The daemon's default idle timeout (`daemon.idle-timeout`, ARCHITECTURE §14.3).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// One registered connection.
#[derive(Debug, Clone)]
pub struct Connection {
    pub peer: Peer,
    pub summary: ConnectionSummary,
    pub interactive: bool,
}

/// Every initialized connection (`daemon.status`, broadcasts, idle detection).
#[derive(Debug, Clone, Default)]
pub struct Connections {
    inner: Arc<Mutex<BTreeMap<String, Connection>>>,
}

impl Connections {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Connection>> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Registers an initialized connection.
    pub fn register(&self, peer: &Peer) {
        let Some(session) = peer.session() else { return };
        let summary = ConnectionSummary {
            id: session.conn.id.clone(),
            kind: session.conn.kind,
            name: session.conn.name.clone(),
            pid: session.client_pid,
        };
        let entry = Connection { peer: peer.clone(), summary, interactive: session.conn.interactive };
        self.lock().insert(session.conn.id.clone(), entry);
    }

    /// Forgets a closed connection.
    pub fn remove(&self, id: &str) {
        self.lock().remove(id);
    }

    /// The connections, in id order.
    pub fn list(&self) -> Vec<ConnectionSummary> {
        let mut out: Vec<ConnectionSummary> = self.lock().values().map(|c| c.summary.clone()).collect();
        out.sort_by_key(|c| c.id.trim_start_matches("c-").parse::<u64>().unwrap_or(u64::MAX));
        out
    }

    /// How many connections are open.
    pub fn count(&self) -> usize {
        self.lock().len()
    }

    /// The peer of connection `id`.
    pub fn peer(&self, id: &str) -> Option<Peer> {
        self.lock().get(id).map(|c| c.peer.clone())
    }

    /// The workspace roots connection `id` named at `initialize` (ARCHITECTURE §7.4).
    pub fn workspace_roots(&self, id: &str) -> Vec<String> {
        self.lock().get(id).and_then(|c| c.peer.session().map(|s| s.workspace_roots.clone())).unwrap_or_default()
    }

    /// Sends a notification to every connection `filter` accepts; never blocks.
    pub fn notify_where(&self, method: &str, params: &Value, filter: impl Fn(&Connection) -> bool) {
        let targets: Vec<Peer> = self.lock().values().filter(|c| filter(c)).map(|c| c.peer.clone()).collect();
        for peer in targets {
            let _ = peer.notify_raw(method, params.clone());
        }
    }

    /// The connections that see approval requests: `ide`, and `cli` with a TTY (ARCHITECTURE §9.1).
    pub fn is_approver(c: &Connection) -> bool {
        c.summary.kind == ClientKind::Ide || (c.summary.kind == ClientKind::Cli && c.interactive)
    }

    /// Closes every connection (shutdown).
    pub fn close_all(&self) {
        for c in self.lock().values() {
            c.peer.close();
        }
    }
}

/// Keeps the daemon from idling out while held (the MCP HTTP endpoint, W20).
#[derive(Debug)]
pub struct KeepAlive(Arc<AtomicUsize>);

impl Drop for KeepAlive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The daemon's identity and lifecycle state (`DaemonCtx.daemon`).
#[derive(Debug)]
pub struct DaemonState {
    pub dirs: RussetDirs,
    pub version: &'static str,
    pub build_id: &'static str,
    pub pid: u32,
    /// RFC 3339 UTC.
    pub started_at: String,
    started: Instant,
    pub executable: PathBuf,
    pub executable_id: ExecutableId,
    /// The device backend (`live`, `scripted:<file>`, …, `none`).
    pub device_backend: String,
    /// `RUSSET_HOME`, when the daemon runs under one.
    pub home: Option<PathBuf>,
    /// The endpoint the daemon serves (`unix:…`, or `inproc`).
    endpoint: Mutex<String>,
    pub connections: Connections,
    pub logs: Arc<LogControl>,
    /// `daemon.idle-timeout`; `None` never idles out.
    idle_timeout: Mutex<Option<Duration>>,
    /// The configuration hooks and what flags pinned ([`settings`]).
    pub settings: settings::SettingsControl,
    /// Whether this daemon holds `runtime/russetd.lock`, and so owns the home's persistent state (`state/jobs`
    /// and the crash recovery of other areas, for example W7's sessions). An in-process daemon of a home whose
    /// daemon runs does not.
    pub owns_home: bool,
    keep_alive: Arc<AtomicUsize>,
    shutdown: CancellationToken,
    reason: Mutex<Option<(ShutdownReason, bool)>>,
}

impl DaemonState {
    /// The state of a daemon started now from this process's executable.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dirs: RussetDirs,
        device_backend: String,
        home: Option<PathBuf>,
        logs: Arc<LogControl>,
        idle_timeout: Option<Duration>,
        settings: settings::SettingsControl,
        owns_home: bool,
        shutdown: CancellationToken,
    ) -> Self {
        let (executable, executable_id) = russet_core::platform::current_executable()
            .unwrap_or_else(|_| (PathBuf::from("russet"), ExecutableId::default()));
        Self {
            dirs,
            version: russet_core::build_info::VERSION,
            build_id: russet_core::build_info::BUILD_ID,
            pid: std::process::id(),
            started_at: russet_core::time::now_rfc3339(),
            started: Instant::now(),
            executable,
            executable_id,
            device_backend,
            home,
            endpoint: Mutex::new("inproc".to_owned()),
            connections: Connections::default(),
            logs,
            idle_timeout: Mutex::new(idle_timeout.filter(|t| !t.is_zero())),
            settings,
            owns_home,
            keep_alive: Arc::new(AtomicUsize::new(0)),
            shutdown,
            reason: Mutex::new(None),
        }
    }

    /// `daemon.idle-timeout` (`None`: never).
    pub fn idle_timeout(&self) -> Option<Duration> {
        *self.idle_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Changes the idle timeout (a reloaded `config.toml`); `None` or zero never idles out.
    pub fn set_idle_timeout(&self, t: Option<Duration>) {
        *self.idle_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = t.filter(|t| !t.is_zero());
    }

    /// The endpoint string.
    pub fn endpoint(&self) -> String {
        self.endpoint.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Records the endpoint once bound.
    pub fn set_endpoint(&self, e: String) {
        *self.endpoint.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = e;
    }

    /// Milliseconds since start.
    pub fn uptime_ms(&self) -> u64 {
        russet_core::time::millis(self.started.elapsed())
    }

    /// `initialize.result.server`.
    pub fn server_info(&self) -> ServerInfo {
        ServerInfo {
            name: "russetd".to_owned(),
            version: self.version.to_owned(),
            build_id: self.build_id.to_owned(),
            pid: self.pid,
            executable: self.executable.display().to_string(),
            executable_id: self.executable_id.into(),
            started_at: self.started_at.clone(),
            platform: russet_core::platform::host_os().to_owned(),
            arch: russet_core::platform::host_arch().to_owned(),
            device_backend: self.device_backend.clone(),
        }
    }

    /// Asks the daemon to stop (the first request's reason wins).
    pub fn request_shutdown(&self, reason: ShutdownReason, force: bool) {
        let mut r = self.reason.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if r.is_none() {
            *r = Some((reason, force));
        }
        drop(r);
        tracing::info!(?reason, force, "shutdown requested");
        self.shutdown.cancel();
    }

    /// Why and how the daemon stops (`request`, not forced, when nothing was recorded).
    pub fn shutdown_reason(&self) -> (ShutdownReason, bool) {
        self.reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unwrap_or((ShutdownReason::Request, false))
    }

    /// Whether shutdown has started.
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    /// Keeps the daemon alive until the guard is dropped.
    pub fn keep_alive(&self) -> KeepAlive {
        self.keep_alive.fetch_add(1, Ordering::SeqCst);
        KeepAlive(Arc::clone(&self.keep_alive))
    }

    /// Whether something holds a [`KeepAlive`].
    pub fn kept_alive(&self) -> bool {
        self.keep_alive.load(Ordering::SeqCst) > 0
    }

    /// Writes `runtime/russetd.json` (0600; ARCHITECTURE §3.3 item 2).
    pub fn write_discovery(&self, web: Option<&str>) -> Result<()> {
        let mut v = serde_json::json!({
            "pid": self.pid,
            "version": self.version,
            "buildId": self.build_id,
            "protocol": russet_protocol::PROTOCOL,
            "endpoint": self.endpoint(),
            "startedAt": self.started_at,
            "deviceBackend": self.device_backend,
        });
        if let (Some(web), Some(m)) = (web, v.as_object_mut()) {
            m.insert("web".to_owned(), Value::String(web.to_owned()));
        }
        let bytes =
            serde_json::to_vec_pretty(&v).map_err(|e| Error::internal(format!("encoding russetd.json: {e}")))?;
        russet_core::fsx::atomic_write(&self.dirs.daemon_json(), &bytes)
    }

    /// Removes `runtime/russetd.json` (shutdown).
    pub fn remove_discovery(&self) {
        let _ = std::fs::remove_file(self.dirs.daemon_json());
    }
}

/// Daemon doctor checks (live-device lock), for the doctor seam; `dirs.private` is russet-doctor's host check.
pub fn checks() -> Vec<std::sync::Arc<dyn russet_doctor::Check>> {
    async_trait_shim::checks()
}

/// The checks themselves; `russet_doctor::Check` is an `async_trait` trait, implemented through the
/// re-exported macro of russet-core.
mod async_trait_shim {
    use std::sync::Arc;

    use russet_core::async_trait;
    use russet_doctor::{Category, Check, CheckCx, CheckResult, Requires, Status};

    use super::backend::{DeviceBackend, live_allowed};
    use super::lock::{live_device_lock_path, read_holder};

    pub(super) fn checks() -> Vec<Arc<dyn Check>> {
        vec![Arc::new(LiveDeviceLock)]
    }

    fn result(status: Status, summary: impl Into<String>, hint: Option<&str>) -> CheckResult {
        CheckResult { status, summary: summary.into(), detail: None, hint: hint.map(str::to_owned) }
    }

    /// `daemon.live-device-lock`: which daemon owns the phone (ARCHITECTURE §3.3 item 7).
    struct LiveDeviceLock;

    #[async_trait]
    impl Check for LiveDeviceLock {
        fn id(&self) -> &'static str {
            "daemon.live-device-lock"
        }
        fn title(&self) -> &'static str {
            "The live device backend and its host-wide lock"
        }
        fn category(&self) -> Category {
            Category::Daemon
        }
        fn requires(&self) -> Requires {
            Requires::default()
        }
        async fn run(&self, _cx: &CheckCx) -> CheckResult {
            let backend = match DeviceBackend::from_env() {
                Ok(b) => b,
                Err(e) => return result(Status::Fail, e.message(), e.hint()),
            };
            if !backend.is_live() {
                return result(Status::Skip, format!("device backend `{backend}`: no live-device lock needed"), None);
            }
            let lane_a = std::env::var("RUSSET_LANE").is_ok_and(|v| v == "A");
            if !live_allowed(russet_core::build_info::is_development(), lane_a, false) {
                return result(
                    Status::Warn,
                    "a development daemon of this environment refuses the live backend (RUSSET_LANE is not A)",
                    Some("Lane D: set RUSSET_DEVICE_BACKEND=scripted:<scenario.toml> or none (AGENTS.md §4)."),
                );
            }
            let path = live_device_lock_path();
            match read_holder(&path) {
                Some(h) if h.pid != std::process::id() => result(
                    Status::Pass,
                    format!("the live-device lock was last taken by pid {} (home {})", h.pid, h.home),
                    None,
                ),
                _ => result(Status::Pass, format!("the live-device lock is {}", path.display()), None),
            }
        }
    }
}
