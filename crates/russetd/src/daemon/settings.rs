// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Daemon settings from the configuration files (ARCHITECTURE §14.1, §14.3, §14.6): `[daemon]` and `[web]` of
//! `config.toml`, and `mcp-policy.toml`.
//!
//! The files belong to other areas: W3's `ConfigService` parses `config.toml` and W13 loads `mcp-policy.toml`.
//! They hand their values to the daemon through two registration hooks, called from their
//! `handlers/<ns>.rs::register` (the handler seam); both get the [`DaemonCtx`] (`cx.config.get()?`, `cx.dirs`):
//! - [`from_config`]: `[daemon] idle-timeout`, `max-parallel-jobs`, `max-build-jobs`, `log-filter`,
//!   `log-retention-days` and `[web] listen`, as [`DaemonSettings`];
//! - [`mcp_policy`]: the parsed [`McpPolicy`].
//!
//! The daemon applies them at start and on SIGHUP, and, once the configuration service has started, whenever
//! it announces a change (`ConfigService::subscribe`).
//! Flags and `RUSSET_*` variables win (§14.1): `--idle-timeout` and `RUSSET_LOG` pin their values. Only daemons
//! whose options come from [`DaemonOptions::from_env`](crate::DaemonOptions::from_env) read the files; daemons
//! built with [`DaemonOptions::new`](crate::DaemonOptions::new) (tests, embedders) run with exactly their
//! options. Without a hook, or when the configuration cannot be read (an invalid `config.toml` is `doctor`'s to
//! report, never a reason not to start), the built-in defaults stay.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use russet_core::error::Result;
use russet_ipc::Router;

use crate::DaemonCtx;
use crate::perms::McpPolicy;

/// What `config.toml` says about the daemon; `None` keeps the current value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonSettings {
    /// `daemon.idle-timeout`; zero never idles out.
    pub idle_timeout: Option<Duration>,
    /// `daemon.max-parallel-jobs` (at least 1).
    pub max_parallel_jobs: Option<usize>,
    /// `daemon.max-build-jobs` (at least 1).
    pub max_build_jobs: Option<usize>,
    /// `daemon.log-filter` (an `EnvFilter` directive).
    pub log_filter: Option<String>,
    /// `daemon.log-retention-days`.
    pub log_retention_days: Option<usize>,
    /// `[web] listen`; it applies while the web endpoint has not started.
    pub web_listen: Option<SocketAddr>,
}

type SettingsFn = Arc<dyn Fn(&DaemonCtx) -> Result<DaemonSettings> + Send + Sync>;
type PolicyFn = Arc<dyn Fn(&DaemonCtx) -> Result<McpPolicy> + Send + Sync>;

/// The hooks registered with [`from_config`] and [`mcp_policy`] (a router extension until the daemon takes it).
#[derive(Clone, Default)]
pub struct ConfigHooks {
    settings: Option<SettingsFn>,
    mcp_policy: Option<PolicyFn>,
}

impl std::fmt::Debug for ConfigHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigHooks")
            .field("settings", &self.settings.is_some())
            .field("mcp_policy", &self.mcp_policy.is_some())
            .finish()
    }
}

/// Registers how the daemon's settings are read from the configuration (W3: from `cx.config`).
pub fn from_config(
    r: &mut Router<DaemonCtx>,
    f: impl Fn(&DaemonCtx) -> Result<DaemonSettings> + Send + Sync + 'static,
) {
    r.extensions_mut().get_or_default::<ConfigHooks>().settings = Some(Arc::new(f));
}

/// Registers how `mcp-policy.toml` is loaded (W13: from `cx.dirs.config`).
pub fn mcp_policy(r: &mut Router<DaemonCtx>, f: impl Fn(&DaemonCtx) -> Result<McpPolicy> + Send + Sync + 'static) {
    r.extensions_mut().get_or_default::<ConfigHooks>().mcp_policy = Some(Arc::new(f));
}

/// Values a flag or an environment variable set, which the configuration does not override (§14.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pinned {
    /// `--idle-timeout`.
    pub idle_timeout: bool,
    /// `RUSSET_LOG`.
    pub log_filter: bool,
}

/// A daemon's settings control (`DaemonState::settings`).
#[derive(Debug, Default)]
pub struct SettingsControl {
    hooks: ConfigHooks,
    read_config: bool,
    pinned: Pinned,
    watching: AtomicBool,
}

impl SettingsControl {
    /// Reads the configuration through `hooks` when `read_config` is set, except what `pinned` names.
    pub fn new(hooks: ConfigHooks, read_config: bool, pinned: Pinned) -> Self {
        Self { hooks, read_config, pinned, watching: AtomicBool::new(false) }
    }
}

/// What one [`reload`] changed (logs and tests).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reloaded {
    pub settings: Option<DaemonSettings>,
    pub mcp_policy: bool,
}

/// Reads the settings and the MCP policy again and applies them (module docs).
pub fn reload(cx: &DaemonCtx) -> Reloaded {
    let control = &cx.daemon.settings;
    let mut out = Reloaded::default();
    if !control.read_config {
        return out;
    }
    if let Some(load) = &control.hooks.mcp_policy {
        match load(cx) {
            Ok(policy) => {
                cx.perms.set_policy(policy);
                out.mcp_policy = true;
            }
            Err(e) => tracing::warn!("mcp-policy.toml is not applied: {}", e.message()),
        }
    }
    let Some(read) = &control.hooks.settings else { return out };
    match read(cx) {
        Ok(s) => {
            apply(cx, &s, control.pinned);
            out.settings = Some(s);
        }
        Err(e) => {
            tracing::warn!("the daemon keeps its settings: the configuration cannot be read: {}", e.message());
        }
    }
    // Once the configuration service runs, follow its changes.
    if let Some(config) = cx.config.try_get()
        && !control.watching.swap(true, Ordering::SeqCst)
    {
        let changes = crate::daemon::panics::catch_expected(|| config.subscribe());
        match changes {
            Ok(changes) => {
                tokio::spawn(watch(cx.clone(), changes));
            }
            Err(_) => tracing::debug!("the configuration service cannot announce changes yet"),
        }
    }
    out
}

/// Re-applies the settings whenever the configuration service announces a change.
async fn watch(cx: DaemonCtx, mut changes: tokio::sync::broadcast::Receiver<russet_config::ConfigChanged>) {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        tokio::select! {
            () = cx.shutdown.cancelled() => return,
            r = changes.recv() => match r {
                Ok(_) | Err(RecvError::Lagged(_)) => {
                    let _ = reload(&cx);
                }
                Err(RecvError::Closed) => return,
            },
        }
    }
}

/// Applies `s` except the pinned values.
pub fn apply(cx: &DaemonCtx, s: &DaemonSettings, pinned: Pinned) {
    if let (Some(t), false) = (s.idle_timeout, pinned.idle_timeout) {
        cx.daemon.set_idle_timeout((!t.is_zero()).then_some(t));
    }
    if s.max_parallel_jobs.is_some() || s.max_build_jobs.is_some() {
        let mut limits = cx.jobs.limits();
        if let Some(n) = s.max_parallel_jobs {
            limits.max_parallel = n.max(1);
        }
        if let Some(n) = s.max_build_jobs {
            limits.max_heavy = n.max(1);
        }
        cx.jobs.set_limits(limits);
    }
    if let (Some(filter), false) = (&s.log_filter, pinned.log_filter)
        && let Err(e) = cx.daemon.logs.set_filter(filter)
    {
        tracing::warn!("daemon.log-filter is not applied: {}", e.message());
    }
    if let Some(days) = s.log_retention_days {
        cx.daemon.logs.set_retention_days(days);
    }
    if let Some(addr) = s.web_listen
        && !cx.web.set_listen(addr)
    {
        tracing::info!("[web] listen applies at the next start: the web endpoint already runs");
    }
    tracing::debug!(?s, "settings applied");
}
