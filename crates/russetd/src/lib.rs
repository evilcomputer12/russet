// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russetd`, the per-user daemon (ARCHITECTURE §3, §5.3 `russetd`, §6): lifecycle, [`DaemonCtx`], handlers,
//! pipelines, permissions, approvals, jobs, leases, subscriptions, the web endpoint and the audit log.
//!
//! W2 owns this file and the runtime modules (CORE-06, CORE-13). [`run`] is `russet daemon run`; [`InProcess`]
//! is the same daemon behind an in-process duplex (`--no-daemon`, tests).
//!
//! **Registration hooks.** The handler seam gives an area one call, `handlers/<ns>.rs::register(r)`
//! (ARCHITECTURE §5.3). Besides its handlers, an area declares there what the daemon needs to know about its
//! methods and services; the values travel as router extensions and the daemon takes them when it starts:
//! - [`jobs::declare`]: a long-running method's job locks, weight, event mirror, kind and title (§6.2);
//! - [`perms::typed_text`]: the text a typed confirmation expects (§7.5);
//! - [`perms::project_bundle_ids`]: the bundle IDs of a connection's projects (`Ownership`, §5.3);
//! - [`ctx::provide`]: the constructor of a `DaemonCtx` service (`config`, `secrets`, `toolchains`, `debug`,
//!   `accounts`);
//! - [`daemon::settings::from_config`] and [`daemon::settings::mcp_policy`]: the daemon's `config.toml` settings
//!   and `mcp-policy.toml` (§14.3, §14.6).

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_ipc::{ClientInfo, Endpoint, Peer, Router};
use russet_protocol::initialize::InitializeResult;

pub mod approvals;
pub mod audit;
pub mod ctx;
pub mod daemon;
pub mod doctor_registry;
pub mod handlers;
pub mod jobs;
pub mod leases;
pub mod perms;
pub mod pipeline;
pub mod state;
pub mod subs;
pub mod web;

pub use ctx::{DaemonCtx, DeviceSlot, Service, ServiceEnv};
pub use daemon::backend::DeviceBackend;
pub use daemon::logging::LogOptions;
pub use daemon::settings::DaemonSettings;
pub use jobs::{JobLimits, JobOptions};
pub use perms::McpPolicy;

/// Extra registrations on the daemon's router after the handler seam (tests, embedders).
pub type Registration = Arc<dyn Fn(&mut Router<DaemonCtx>) + Send + Sync>;

/// How to run the daemon (`russet daemon run`).
pub struct DaemonOptions {
    pub dirs: RussetDirs,
    pub endpoint: Endpoint,
    /// `daemon.idle-timeout` (default 30 min); `None` or zero never idles out.
    pub idle_timeout: Option<Duration>,
    /// `--idle-timeout` was given: `daemon.idle-timeout` does not override it (ARCHITECTURE §14.1).
    pub idle_timeout_pinned: bool,
    /// Read `config.toml` and `mcp-policy.toml` through the areas' hooks ([`daemon::settings`]). `from_env`
    /// sets it; daemons built with [`DaemonOptions::new`] run with exactly their options.
    pub read_config: bool,
    /// Log to stderr too.
    pub foreground: bool,
    /// `--allow-live-device`: development builds otherwise refuse the `live` backend without `RUSSET_LANE=A`
    /// (ARCHITECTURE §3.3 item 7).
    pub allow_live_device: bool,
    /// `RUSSET_DEVICE_BACKEND` (ARCHITECTURE §14.9).
    pub device_backend: DeviceBackend,
    /// `RUSSET_LANE=A`.
    pub lane_a: bool,
    /// The host-wide live-device lock (`RUSSET_LIVE_DEVICE_LOCK` in debug builds).
    pub live_device_lock: PathBuf,
    /// `RUSSET_HOME`, recorded as the lock holder's home.
    pub home: Option<PathBuf>,
    /// `--detach`: start a new session (no controlling terminal).
    pub detach: bool,
    pub log: LogOptions,
    pub limits: JobLimits,
    /// The MCP policy (`mcp-policy.toml`; its loader is W13's).
    pub mcp_policy: McpPolicy,
    /// `[web] listen` (`127.0.0.1:0`).
    pub web_listen: SocketAddr,
    /// Registrations after the handler seam's, in order.
    pub registrations: Vec<Registration>,
}

impl std::fmt::Debug for DaemonOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonOptions")
            .field("dirs", &self.dirs)
            .field("endpoint", &self.endpoint)
            .field("idle_timeout", &self.idle_timeout)
            .field("idle_timeout_pinned", &self.idle_timeout_pinned)
            .field("read_config", &self.read_config)
            .field("foreground", &self.foreground)
            .field("allow_live_device", &self.allow_live_device)
            .field("device_backend", &self.device_backend)
            .field("lane_a", &self.lane_a)
            .field("live_device_lock", &self.live_device_lock)
            .field("home", &self.home)
            .field("detach", &self.detach)
            .field("log", &self.log)
            .field("limits", &self.limits)
            .field("mcp_policy", &self.mcp_policy)
            .field("web_listen", &self.web_listen)
            .field("registrations", &self.registrations.len())
            .finish()
    }
}

impl DaemonOptions {
    /// Options for a daemon of `dirs` with the defaults and **no device plane** (`none`): what tests and
    /// in-process daemons want unless they say otherwise.
    pub fn new(dirs: RussetDirs) -> Self {
        let endpoint = Endpoint::default_for(&dirs);
        Self {
            dirs,
            endpoint,
            idle_timeout: Some(daemon::DEFAULT_IDLE_TIMEOUT),
            idle_timeout_pinned: false,
            read_config: false,
            foreground: false,
            allow_live_device: false,
            device_backend: DeviceBackend::None,
            lane_a: false,
            live_device_lock: daemon::lock::live_device_lock_path(),
            home: None,
            detach: false,
            log: LogOptions::default(),
            limits: JobLimits::default(),
            mcp_policy: McpPolicy::default(),
            web_listen: SocketAddr::from(([127, 0, 0, 1], 0)),
            registrations: Vec::new(),
        }
    }

    /// Options from the environment: `RUSSET_DAEMON`, `RUSSET_DEVICE_BACKEND` (default `live`), `RUSSET_LANE`,
    /// `RUSSET_LIVE_DEVICE_LOCK` (debug builds), `RUSSET_HOME`, `RUSSET_LOG`, `RUSSET_LOG_FORMAT`; the daemon
    /// also reads `config.toml` and `mcp-policy.toml` ([`DaemonOptions::read_config`]).
    pub fn from_env(dirs: RussetDirs) -> Result<Self> {
        let mut o = Self::new(dirs);
        o.read_config = true;
        o.endpoint = Endpoint::from_env(&o.dirs)?;
        o.device_backend = DeviceBackend::from_env()?;
        o.lane_a = std::env::var("RUSSET_LANE").is_ok_and(|v| v == "A");
        o.home = std::env::var_os("RUSSET_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
        o.log = LogOptions::from_env();
        Ok(o)
    }
}

/// Runs the daemon until shutdown.
pub async fn run(opts: DaemonOptions) -> Result<()> {
    daemon::run::run(opts).await
}

/// An in-process daemon: `--no-daemon` and tests.
pub struct InProcess {
    daemon: daemon::run::InProcessDaemon,
}

impl std::fmt::Debug for InProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcess").field("endpoint", &"inproc").finish()
    }
}

impl InProcess {
    /// Starts the daemon in this process and connects a `test` client to it.
    pub async fn start(o: DaemonOptions) -> Result<(InProcess, Peer)> {
        let client = ClientInfo::new(russet_core::rpc::ClientKind::Test, "in-process");
        let (daemon, peer, _) = Self::start_with(o, &client).await?;
        Ok((daemon, peer))
    }

    /// Starts the daemon in this process and connects `client` to it.
    pub async fn start_with(o: DaemonOptions, client: &ClientInfo) -> Result<(InProcess, Peer, InitializeResult)> {
        let daemon = daemon::run::InProcessDaemon::new(&o)?;
        let (peer, result) = daemon.connect(client).await?;
        Ok((Self { daemon }, peer, result))
    }

    /// Another connection to the same daemon.
    pub async fn connect(&self, client: &ClientInfo) -> Result<(Peer, InitializeResult)> {
        self.daemon.connect(client).await
    }

    /// A connection that has not sent `initialize` yet (tests of the handshake).
    pub fn connect_raw(&self) -> Peer {
        self.daemon.connect_raw()
    }

    /// The daemon's context (tests).
    pub fn ctx(&self) -> &DaemonCtx {
        &self.daemon.ctx
    }

    /// The ownership queries the permission engine uses (`Ownership`, ARCHITECTURE §5.3; tests).
    pub fn ownership(&self) -> &dyn russet_core::rpc::Ownership {
        self.daemon.ownership()
    }

    /// Stops the daemon like `daemon.shutdown`.
    pub async fn stop(self) -> Result<()> {
        self.daemon.stop().await;
        Ok(())
    }
}
