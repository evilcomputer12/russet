// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet daemon run` and the in-process daemon (ARCHITECTURE §3.3; core §6.3).
//!
//! Start: take `runtime/russetd.lock` (another daemon of this home makes this one exit with
//! `ipc.daemonUnavailable`, exit 12), set up logging and the panic hook, recover jobs, decide the live-device
//! state, bind the endpoint (a stale socket is replaced), write `runtime/russetd.json`.
//!
//! Stop (`daemon.shutdown`, SIGTERM/SIGINT, idle exit): stop accepting, broadcast `daemon.shuttingDown`,
//! cancel jobs with 10 s of grace (none with `force`) and kill what is left, end leases and subscriptions,
//! close connections, remove `russetd.json`, release the lock.
//!
//! Idle exit: after `idle-timeout` with no connections, no queued or running jobs, no detached leases and
//! nothing holding a keep-alive (the MCP HTTP endpoint).

use std::sync::Arc;
use std::time::{Duration, Instant};

use russet_core::error::{Error, Result};
use russet_ipc::{ClientInfo, ListenOptions, Peer, Router, Server, in_process, serve_peer};
use russet_protocol::NotificationMethod;
use russet_protocol::daemon::{DaemonShuttingDown, DaemonShuttingDownParams, ShutdownReason};
use russet_protocol::initialize::InitializeResult;
use tokio_util::sync::CancellationToken;

use super::backend::live_allowed;
use super::lock::{FileLock, LockHolder, already_running, take_live_device_lock};
use super::logging::LogControl;
use super::{DaemonState, SHUTDOWN_GRACE};
use crate::approvals::ApprovalService;
use crate::audit::AuditLog;
use crate::ctx::{DaemonCtx, DeviceSlot, LiveRefusal, LiveState, Service};
use crate::jobs::JobManager;
use crate::jobs::layer::DaemonLayer;
use crate::leases::LeaseTable;
use crate::perms::{DaemonAuthorizer, DaemonOwnership, PermissionEngine};
use crate::subs::SubscriptionHub;
use crate::web::WebEndpoint;
use crate::{DaemonOptions, handlers};

/// How long a starting daemon waits for the lock of a daemon that is still shutting down.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// A built daemon: its context, router and authorizer.
pub(crate) struct Built {
    pub ctx: DaemonCtx,
    pub router: Arc<Router<DaemonCtx>>,
    pub auth: Arc<DaemonAuthorizer>,
}

fn secrets_config() -> russet_secrets::SecretsConfig {
    use russet_secrets::BackendChoice;
    let backend = match std::env::var("RUSSET_SECRETS_BACKEND").unwrap_or_default().as_str() {
        "keyring" => BackendChoice::Keyring,
        "passphrase" => BackendChoice::Passphrase,
        "env" => BackendChoice::Env,
        "memory" => BackendChoice::Memory,
        _ => BackendChoice::Auto,
    };
    russet_secrets::SecretsConfig { backend }
}

/// Assembles the context: the live-device state, the services, the router with every namespace's handlers
/// and the daemon layer, and the authorizer.
pub(crate) fn build(opts: &DaemonOptions, logs: Arc<LogControl>, shutdown: CancellationToken) -> Result<Built> {
    let dirs = opts.dirs.clone();
    dirs.ensure()?;
    let backend = opts.device_backend.clone();
    let live = if backend.is_live() {
        if live_allowed(russet_core::build_info::is_development(), opts.lane_a, opts.allow_live_device) {
            let me = LockHolder {
                pid: std::process::id(),
                home: opts.home.as_ref().unwrap_or(&dirs.runtime).display().to_string(),
            };
            match take_live_device_lock(&opts.live_device_lock, &me)? {
                Ok(lock) => {
                    tracing::info!(lock = %opts.live_device_lock.display(), "this daemon owns the live device");
                    LiveState::Held(lock)
                }
                Err(holder) => {
                    tracing::warn!(?holder, "another daemon holds the live-device lock");
                    LiveState::Refused(LiveRefusal::HeldElsewhere(holder))
                }
            }
        } else {
            tracing::info!("development daemon without RUSSET_LANE=A: the live backend is refused");
            LiveState::Refused(LiveRefusal::DevelopmentBuild)
        }
    } else {
        LiveState::NotLive
    };
    let devices = DeviceSlot::new(backend.clone(), live, dirs.state.join("devices"));
    let audit = AuditLog::new(dirs.audit_log());
    let state = Arc::new(DaemonState::new(
        dirs.clone(),
        backend.to_string(),
        opts.home.clone(),
        logs,
        opts.idle_timeout,
        shutdown.clone(),
    ));
    let approvals = ApprovalService::new(state.connections.clone(), audit.clone());
    let jobs = JobManager::new(Some(dirs.jobs()), opts.limits, shutdown.clone());
    let leases = LeaseTable::default();
    let perms = Arc::new(PermissionEngine::new(opts.mcp_policy.clone()));
    let web = WebEndpoint::new(opts.web_listen, shutdown.clone());
    {
        let state = Arc::clone(&state);
        web.on_start(move |url| {
            if let Err(e) = state.write_discovery(Some(url)) {
                tracing::warn!("cannot update russetd.json: {}", e.message());
            }
        });
    }

    let config = {
        let dirs = dirs.clone();
        Service::lazy("config", move || Ok(Arc::new(russet_config::ConfigService::load(&dirs)?)))
    };
    let secrets: Service<dyn russet_secrets::SecretStore> = {
        let dirs = dirs.clone();
        Service::lazy("secrets", move || russet_secrets::open_default(&dirs, &secrets_config()))
    };
    let toolchains = Service::unavailable("toolchains", || {
        Error::not_implemented("russet-toolchain has no constructor yet (W3, CORE-10)")
            .with_hint("The toolchain service lands with W3; see docs/PLAN.md §4.3.")
    });
    let debug = {
        let devices = devices.clone();
        Service::lazy("debug", move || {
            let services: Arc<dyn russet_debug::DeviceServices> = Arc::new(devices.clone());
            Ok(Arc::new(russet_debug::DebugService::new(services, russet_debug::DebugConfig::default())))
        })
    };
    let accounts = {
        let (dirs, secrets) = (dirs.clone(), secrets.clone());
        Service::lazy("accounts", move || {
            let store = secrets.get()?;
            let endpoints = russet_core::net::Endpoints::from_env()?;
            Ok(Arc::new(russet_account::AccountService::new(&dirs, store, endpoints)?))
        })
    };

    let ownership = DaemonOwnership { jobs: jobs.clone(), leases: leases.clone() };
    let ctx = DaemonCtx {
        dirs,
        config,
        secrets,
        toolchains,
        jobs,
        leases,
        subs: SubscriptionHub::default(),
        perms: Arc::clone(&perms),
        approvals: approvals.clone(),
        audit: audit.clone(),
        web,
        devices: devices.clone(),
        debug,
        accounts,
        shutdown,
        daemon: state,
    };
    let mut router = Router::new();
    handlers::register_all(&mut router);
    router.layer(Arc::new(DaemonLayer { ownership: ownership.clone() }));
    let auth = Arc::new(DaemonAuthorizer { engine: perms, approvals, devices, ownership, audit });
    Ok(Built { ctx, router: Arc::new(router), auth })
}

/// Stops a daemon: broadcast, jobs with grace, leases, subscriptions, connections, discovery file.
pub(crate) async fn stop(ctx: &DaemonCtx) {
    let (reason, force) = ctx.daemon.shutdown_reason();
    let grace = if force { Duration::ZERO } else { SHUTDOWN_GRACE };
    let params = DaemonShuttingDownParams { reason, grace_ms: russet_core::time::millis(grace) };
    if let Ok(v) = serde_json::to_value(params) {
        ctx.daemon.connections.notify_where(DaemonShuttingDown::NAME, &v, |_| true);
    }
    tracing::info!(?reason, force, "shutting down");
    ctx.jobs.shutdown(grace).await;
    ctx.leases.release_all();
    ctx.subs.close_all();
    // Let the last notifications and replies reach the clients.
    tokio::time::sleep(Duration::from_millis(50)).await;
    ctx.daemon.connections.close_all();
    ctx.devices.release_lock();
    ctx.daemon.remove_discovery();
}

/// Requests an idle shutdown after `idle-timeout` without connections, jobs, detached leases or keep-alives.
async fn idle_watch(ctx: DaemonCtx) {
    let Some(timeout) = ctx.daemon.idle_timeout.filter(|t| !t.is_zero()) else { return };
    let tick = (timeout / 4).clamp(Duration::from_millis(50), Duration::from_secs(1));
    let mut idle_since: Option<Instant> = None;
    loop {
        tokio::select! {
            () = ctx.shutdown.cancelled() => return,
            () = tokio::time::sleep(tick) => {}
        }
        let counts = ctx.jobs.counts();
        let busy = ctx.daemon.connections.count() > 0
            || counts.queued + counts.running > 0
            || ctx.leases.detached_count() > 0
            || ctx.daemon.kept_alive();
        if busy {
            idle_since = None;
            continue;
        }
        let since = *idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= timeout {
            ctx.daemon.request_shutdown(ShutdownReason::Idle, false);
            return;
        }
    }
}

#[cfg(unix)]
async fn watch_signals(ctx: DaemonCtx) {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int), Ok(mut hup)) =
        (signal(SignalKind::terminate()), signal(SignalKind::interrupt()), signal(SignalKind::hangup()))
    else {
        tracing::warn!("cannot install signal handlers");
        return;
    };
    loop {
        tokio::select! {
            () = ctx.shutdown.cancelled() => return,
            _ = term.recv() => break,
            _ = int.recv() => break,
            _ = hup.recv() => tracing::info!("SIGHUP: configuration reload comes with the config service (W3)"),
        }
    }
    ctx.daemon.request_shutdown(ShutdownReason::Signal, false);
}

#[cfg(windows)]
async fn watch_signals(ctx: DaemonCtx) {
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::select! {
        () = ctx.shutdown.cancelled() => return,
        _ = ctrl_c => {}
    }
    ctx.daemon.request_shutdown(ShutdownReason::Signal, false);
}

/// Takes `runtime/russetd.lock`, waiting up to [`LOCK_WAIT`] for a daemon that is still stopping.
async fn take_daemon_lock(opts: &DaemonOptions) -> Result<FileLock> {
    let path = opts.dirs.daemon_lock();
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        if let Some(lock) = FileLock::try_take(&path)? {
            return Ok(lock);
        }
        if Instant::now() >= deadline {
            return Err(already_running(&path));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Runs the daemon until shutdown (module docs).
pub async fn run(opts: DaemonOptions) -> Result<()> {
    if opts.detach {
        russet_core::platform::detach_from_terminal();
    }
    opts.dirs.ensure()?;
    let lock = take_daemon_lock(&opts).await?;
    let mut log_opts = opts.log.clone();
    log_opts.stderr = log_opts.stderr || opts.foreground;
    let (logs, log_guard) = match super::logging::init_daemon(&opts.dirs, &log_opts) {
        Ok((c, g)) => (c, Some(g)),
        Err(e) => {
            // A test harness or an embedding program already installed a subscriber.
            tracing::debug!("daemon logging not installed: {}", e.message());
            (LogControl::detached(log_opts.filter.as_deref().unwrap_or(super::logging::DEFAULT_FILTER)), None)
        }
    };
    super::panics::install_hook(opts.dirs.panics());
    let shutdown = CancellationToken::new();
    let Built { ctx, router, auth } = build(&opts, logs, shutdown.clone())?;
    let server = Server::bind(&opts.endpoint, ListenOptions::default()).await?;
    ctx.daemon.set_endpoint(server.endpoint());
    ctx.daemon.write_discovery(None)?;
    tracing::info!(
        endpoint = %ctx.daemon.endpoint(),
        build = ctx.daemon.build_id,
        backend = %ctx.daemon.device_backend,
        "russetd started"
    );
    tokio::spawn(watch_signals(ctx.clone()));
    tokio::spawn(idle_watch(ctx.clone()));
    let accepting = CancellationToken::new();
    let serving = tokio::spawn(server.serve(router, ctx.clone(), auth, accepting.clone()));
    shutdown.cancelled().await;
    accepting.cancel();
    match serving.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("the listener stopped with an error: {}", e.message()),
        Err(e) => tracing::warn!("the listener task failed: {e}"),
    }
    stop(&ctx).await;
    tracing::info!("russetd stopped");
    drop(log_guard);
    drop(lock);
    Ok(())
}

/// An in-process daemon (`--no-daemon` and tests): the same context, router and authorizer behind
/// [`in_process`], without a socket, a lock file or global logging.
pub struct InProcessDaemon {
    pub(crate) ctx: DaemonCtx,
    router: Arc<Router<DaemonCtx>>,
    auth: Arc<DaemonAuthorizer>,
}

impl InProcessDaemon {
    pub(crate) fn new(opts: &DaemonOptions) -> Result<Self> {
        let logs = LogControl::detached(opts.log.filter.as_deref().unwrap_or(super::logging::DEFAULT_FILTER));
        let Built { ctx, router, auth } = build(opts, logs, CancellationToken::new())?;
        Ok(Self { ctx, router, auth })
    }

    /// A new client connection, not initialized yet.
    pub(crate) fn connect_raw(&self) -> Peer {
        let (c, s) = in_process();
        let auth: Arc<dyn russet_ipc::Authorizer> = self.auth.clone();
        serve_peer(&s, Arc::clone(&self.router), self.ctx.clone(), auth);
        c
    }

    /// A new client connection, initialized as `client`.
    pub(crate) async fn connect(&self, client: &ClientInfo) -> Result<(Peer, InitializeResult)> {
        let c = self.connect_raw();
        let result = russet_ipc::initialize(&c, client, None).await?;
        Ok((c, result))
    }

    pub(crate) async fn stop(&self) {
        self.ctx.daemon.request_shutdown(ShutdownReason::Request, false);
        stop(&self.ctx).await;
    }
}
