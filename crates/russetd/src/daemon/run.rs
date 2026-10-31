// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet daemon run` and the in-process daemon (ARCHITECTURE §3.3; core §6.3).
//!
//! Start: take `runtime/russetd.lock` (another daemon of this home makes this one exit with
//! `ipc.daemonUnavailable`, exit 12), set up logging and the panic hook, build the router (the handler seam and
//! the areas' registration hooks), recover jobs, decide the live-device state, read the settings, bind the
//! endpoint (a stale socket is replaced), write `runtime/russetd.json`.
//!
//! Stop (`daemon.shutdown`, SIGTERM/SIGINT, idle exit): broadcast `daemon.shuttingDown`, cancel jobs with 10 s
//! of grace (none with `force`) and kill what is left, end leases and subscriptions, close connections, remove
//! `russetd.json`, then stop listening, remove the socket and release the lock (core §6.3). The endpoint stays
//! until the jobs have had their grace: a client that connects meanwhile gets `ipc.daemonUnavailable` with
//! `details.shuttingDown` from `initialize`, waits for this daemon to go and starts a new one, instead of
//! starting a daemon that cannot take the lock.
//!
//! Idle exit: after `idle-timeout` with no connections, no queued or running jobs, no detached leases and
//! nothing holding a keep-alive (the MCP HTTP endpoint).
//!
//! An in-process daemon (`--no-daemon`, tests) runs the same context without a socket. It owns the home's
//! persistent state only if it can take `runtime/russetd.lock` (and then holds it until it stops): otherwise
//! a daemon of the same home runs, and the in-process one keeps its jobs in memory and recovers nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use russet_core::error::Result;
use russet_ipc::{ClientInfo, ListenOptions, Peer, Router, Server, in_process, serve_peer};
use russet_protocol::NotificationMethod;
use russet_protocol::daemon::{DaemonShuttingDown, DaemonShuttingDownParams, ShutdownReason};
use russet_protocol::initialize::InitializeResult;
use tokio_util::sync::CancellationToken;

use super::backend::live_allowed;
use super::lock::{FileLock, LockHolder, already_running, take_live_device_lock};
use super::logging::LogControl;
use super::settings::{ConfigHooks, Pinned, SettingsControl};
use super::{DaemonState, SHUTDOWN_GRACE};
use crate::approvals::ApprovalService;
use crate::audit::AuditLog;
use crate::ctx::{DaemonCtx, DeviceSlot, LiveRefusal, LiveState, ServiceFactories, Services};
use crate::jobs::JobManager;
use crate::jobs::declarations::JobDeclarations;
use crate::jobs::layer::DaemonLayer;
use crate::leases::LeaseTable;
use crate::perms::{BundleIdResolver, DaemonAuthorizer, DaemonOwnership, PermissionEngine, TypedTexts};
use crate::subs::SubscriptionHub;
use crate::web::WebEndpoint;
use crate::{DaemonOptions, handlers};

/// How long a starting daemon waits for the lock of a daemon that is still shutting down.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// How often the daemon deletes log files past their retention.
const LOG_SWEEP_EVERY: Duration = Duration::from_secs(6 * 3600);

/// A built daemon: its context, router and authorizer.
pub(crate) struct Built {
    pub ctx: DaemonCtx,
    pub router: Arc<Router<DaemonCtx>>,
    pub auth: Arc<DaemonAuthorizer>,
}

/// The router with every namespace's handlers and the extra registrations, and what they declared.
struct Registered {
    router: Router<DaemonCtx>,
    factories: ServiceFactories,
    declarations: JobDeclarations,
    typed: TypedTexts,
    bundle_ids: Option<BundleIdResolver>,
    config_hooks: ConfigHooks,
}

fn register(opts: &DaemonOptions) -> Registered {
    let mut router = Router::new();
    handlers::register_all(&mut router);
    for extra in &opts.registrations {
        extra(&mut router);
    }
    let ext = router.extensions_mut();
    let factories = ext.remove::<ServiceFactories>().unwrap_or_default();
    let declarations = ext.remove::<JobDeclarations>().unwrap_or_default();
    let typed = ext.remove::<TypedTexts>().unwrap_or_default();
    let bundle_ids = ext.remove::<BundleIdResolver>();
    let config_hooks = ext.remove::<ConfigHooks>().unwrap_or_default();
    Registered { router, factories, declarations, typed, bundle_ids, config_hooks }
}

/// The holder record written into `runtime/russetd.lock` (for the error of the next daemon).
fn record_home_lock(lock: &mut FileLock, in_process: bool) {
    let record = serde_json::json!({"pid": std::process::id(), "inProcess": in_process});
    if let Err(e) = lock.write_contents(record.to_string().as_bytes()) {
        tracing::debug!("cannot record the daemon lock's holder: {}", e.message());
    }
}

/// Assembles the context: the router with every namespace's handlers and the daemon layer, the live-device
/// state, the services, and the authorizer. `owns_home`: this daemon holds `runtime/russetd.lock`, so it
/// persists and recovers jobs.
pub(crate) fn build(
    opts: &DaemonOptions,
    logs: Arc<LogControl>,
    shutdown: CancellationToken,
    owns_home: bool,
) -> Result<Built> {
    let dirs = opts.dirs.clone();
    dirs.ensure()?;
    let Registered { mut router, factories, declarations, typed, bundle_ids, config_hooks } = register(opts);
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
    let devices = DeviceSlot::new(backend.clone(), live, dirs.clone());
    let services = Services::wire(&dirs, &devices, factories);
    let audit = AuditLog::new(dirs.audit_log());
    let pinned = Pinned { idle_timeout: opts.idle_timeout_pinned, log_filter: opts.log.filter.is_some() };
    let state = Arc::new(DaemonState::new(
        dirs.clone(),
        backend.to_string(),
        opts.home.clone(),
        logs,
        opts.idle_timeout,
        SettingsControl::new(config_hooks, opts.read_config, pinned),
        owns_home,
        shutdown.clone(),
    ));
    let approvals = ApprovalService::new(state.connections.clone(), audit.clone());
    if !owns_home {
        tracing::info!("another daemon owns this home: jobs stay in memory and nothing is recovered");
    }
    let jobs = JobManager::new(owns_home.then(|| dirs.jobs()), opts.limits, shutdown.clone());
    let leases = LeaseTable::default();
    let perms = Arc::new(PermissionEngine::new(opts.mcp_policy.clone()));
    perms.install_typed_texts(typed);
    let web = WebEndpoint::new(opts.web_listen, shutdown.clone());
    {
        let state = Arc::clone(&state);
        web.on_start(move |url| {
            if let Err(e) = state.write_discovery(Some(url)) {
                tracing::warn!("cannot update russetd.json: {}", e.message());
            }
        });
    }

    let ownership = DaemonOwnership {
        jobs: jobs.clone(),
        leases: leases.clone(),
        connections: state.connections.clone(),
        bundle_ids,
    };
    let ctx = DaemonCtx {
        dirs,
        config: services.config,
        secrets: services.secrets,
        toolchains: services.toolchains,
        jobs,
        leases,
        subs: SubscriptionHub::default(),
        perms: Arc::clone(&perms),
        approvals: approvals.clone(),
        audit: audit.clone(),
        web,
        devices: devices.clone(),
        debug: services.debug,
        accounts: services.accounts,
        shutdown,
        daemon: state,
    };
    router.layer(Arc::new(DaemonLayer { ownership: ownership.clone(), declarations: Arc::new(declarations) }));
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
/// The timeout is read on every tick, so a reloaded `daemon.idle-timeout` applies.
async fn idle_watch(ctx: DaemonCtx) {
    let mut idle_since: Option<Instant> = None;
    loop {
        let timeout = ctx.daemon.idle_timeout();
        let tick = timeout
            .map_or(Duration::from_secs(1), |t| (t / 4).clamp(Duration::from_millis(50), Duration::from_secs(1)));
        tokio::select! {
            () = ctx.shutdown.cancelled() => return,
            () = tokio::time::sleep(tick) => {}
        }
        let Some(timeout) = ctx.daemon.idle_timeout() else {
            idle_since = None;
            continue;
        };
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
            _ = hup.recv() => {
                let r = super::settings::reload(&ctx);
                tracing::info!(settings = r.settings.is_some(), mcp_policy = r.mcp_policy, "SIGHUP: settings reloaded");
            }
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

/// Deletes log files past their retention every [`LOG_SWEEP_EVERY`].
async fn log_maintenance(ctx: DaemonCtx) {
    loop {
        tokio::select! {
            () = ctx.shutdown.cancelled() => return,
            () = tokio::time::sleep(LOG_SWEEP_EVERY) => {
                ctx.daemon.logs.sweep();
            }
        }
    }
}

/// Takes `runtime/russetd.lock`, waiting up to [`LOCK_WAIT`] for a daemon that is still stopping.
async fn take_daemon_lock(opts: &DaemonOptions) -> Result<FileLock> {
    let path = opts.dirs.daemon_lock();
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        if let Some(mut lock) = FileLock::try_take(&path)? {
            record_home_lock(&mut lock, false);
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
    let Built { ctx, router, auth } = build(&opts, logs, shutdown.clone(), true)?;
    let reloaded = super::settings::reload(&ctx);
    if reloaded != super::settings::Reloaded::default() {
        tracing::info!(settings = ?reloaded.settings, mcp_policy = reloaded.mcp_policy, "configuration applied");
    }
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
    tokio::spawn(log_maintenance(ctx.clone()));
    let accepting = CancellationToken::new();
    let serving = tokio::spawn(server.serve(router, ctx.clone(), auth, accepting.clone()));
    shutdown.cancelled().await;
    // The endpoint stays while the jobs have their grace (module docs); then the listener closes and the
    // socket goes, and the lock is released last (core §6.3).
    stop(&ctx).await;
    accepting.cancel();
    match serving.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("the listener stopped with an error: {}", e.message()),
        Err(e) => tracing::warn!("the listener task failed: {e}"),
    }
    // Connections accepted during the stop were refused at `initialize`; close any that remain.
    ctx.daemon.connections.close_all();
    tracing::info!("russetd stopped");
    drop(log_guard);
    drop(lock);
    Ok(())
}

/// An in-process daemon (`--no-daemon` and tests): the same context, router and authorizer behind
/// [`in_process`], without a socket or global logging. It holds `runtime/russetd.lock` when it can (module
/// docs).
pub struct InProcessDaemon {
    pub(crate) ctx: DaemonCtx,
    router: Arc<Router<DaemonCtx>>,
    auth: Arc<DaemonAuthorizer>,
    home_lock: std::sync::Mutex<Option<FileLock>>,
}

impl InProcessDaemon {
    pub(crate) fn new(opts: &DaemonOptions) -> Result<Self> {
        opts.dirs.ensure()?;
        let mut home_lock = FileLock::try_take(&opts.dirs.daemon_lock())?;
        if let Some(lock) = home_lock.as_mut() {
            record_home_lock(lock, true);
        }
        let logs = LogControl::detached(opts.log.filter.as_deref().unwrap_or(super::logging::DEFAULT_FILTER));
        let Built { ctx, router, auth } = build(opts, logs, CancellationToken::new(), home_lock.is_some())?;
        let _ = super::settings::reload(&ctx);
        Ok(Self { ctx, router, auth, home_lock: std::sync::Mutex::new(home_lock) })
    }

    /// The ownership queries of the daemon's authorizer.
    pub(crate) fn ownership(&self) -> &DaemonOwnership {
        &self.auth.ownership
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
        drop(self.home_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take());
    }
}
