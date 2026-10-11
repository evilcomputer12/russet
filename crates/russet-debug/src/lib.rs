// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Run and debug sessions (ARCHITECTURE §5.3 `russet-debug`; RD §17.2): launch strategies, the gdb-remote
//! broker, the JIT servicer, LLDB plans and the Swift overlay, the DAP hub, symbols, crashes, the logs view and
//! Flutter run/debug.
//!
//! [`SessionState`] and [`SessionSummary`] convert to the wire shapes of ARCHITECTURE §9.5 (state, lease,
//! debugger) [D 84]. F0 stub (W1); W7 fills it (Wave 1), W11 (Wave 2), W18 (Wave 3); W30 owns [`wifi`].

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use russet_core::Result;
use russet_core::ids::SessionId;
use tokio::sync::broadcast;

pub mod broker;
pub mod crash;
pub mod dap;
pub mod flutter;
pub mod jit;
pub mod launch;
pub mod lldb;
pub mod logs;
pub mod symbols;
pub mod wifi;

/// What russet-debug needs from the device plane (RD §17.3). Implemented here for
/// `russet_device::DeviceManager`, and by russetd's `DeviceSlot`, which starts the manager on first use and
/// delegates to it [D 83]. W7 adds the methods.
pub trait DeviceServices: Send + Sync {}

impl DeviceServices for russet_device::DeviceManager {}

/// Debug-plane configuration (LLDB flavor, attach mode, JIT policy, symbols). W7 defines the fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebugConfig {}

/// `run.start` without a debugger. W7 defines the fields (RD §17.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunRequest {}

/// `debug.prepare`: a suspended launch plus the broker attach. W7 defines the fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebugRequest {}

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

/// Session states; the wire values are the kebab-case `SessionState` of ARCHITECTURE §9.5. Whether a debugger
/// is attached is `SessionSummary.debugger`, not a state [D 84].
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

/// A session event. W7 and W11 add the variants (output, JIT stats, crash, progress, warnings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    State(SessionState),
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

/// One session as `run.list` reports it (ARCHITECTURE §9.5 `SessionSummary`). W7 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: SessionId,
}

/// A handle on a live session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    _f0: (),
}

impl SessionHandle {
    pub fn state(&self) -> SessionState {
        todo!("W7 (RD-04)")
    }
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        todo!("W7 (RD-04)")
    }
    pub async fn terminate(&self, how: TerminateMode) -> Result<()> {
        todo!("W7 (RD-04)")
    }
    pub async fn hot_reload(&self, full: bool) -> Result<HotReloadResult> {
        todo!("W11 (Swift) / W18 (Flutter)")
    }
}

/// A prepared debug session: the app is launched suspended and the broker holds the attach.
#[derive(Debug, Clone)]
pub struct PreparedDebug {
    pub session: SessionHandle,
    pub pid: u32,
    pub broker: BrokerEndpoint,
    pub lldb: LldbLaunchPlan,
    pub dap: Option<DapEndpoint>,
}

/// The session registry (`DaemonCtx.debug`).
#[derive(Debug)]
pub struct DebugService {
    _f0: (),
}

impl DebugService {
    pub fn new(dev: Arc<dyn DeviceServices>, cfg: DebugConfig) -> Self {
        todo!("W7 (RD-04)")
    }
    /// Launches without a debugger.
    pub async fn run(&self, r: RunRequest) -> Result<SessionHandle> {
        todo!("W7 (RD-04)")
    }
    /// Suspended launch plus the broker attach.
    pub async fn prepare_debug(&self, r: DebugRequest) -> Result<PreparedDebug> {
        todo!("W7 (RD-06)")
    }
    pub async fn attach(&self, r: AttachRequest) -> Result<PreparedDebug> {
        todo!("W11")
    }
    pub fn session(&self, id: &SessionId) -> Option<SessionHandle> {
        todo!("W7 (RD-04)")
    }
    pub fn sessions(&self) -> Vec<SessionSummary> {
        todo!("W7 (RD-04)")
    }
}

/// Debug doctor checks (LLDB flavor, libpython env), for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
