// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The JSON-RPC engine (ARCHITECTURE §5.3 `russet-ipc`, §7; core §5.2): LSP codec, [`Peer`], [`Router`],
//! [`Authorizer`], Unix/pipe/TCP transports, the in-process duplex and the autostart client.
//!
//! - **Framing** ([`codec`]): LSP `Content-Length` frames of at most 16 MiB, which stock `vscode-jsonrpc` 9.0.3
//!   speaks (ARCHITECTURE §7.2).
//! - **Connections** ([`Peer`]): requests in both directions, `$/progress`, `$/cancelRequest` (`-32800`),
//!   server-originated ids `s:<n>`.
//! - **Serving** ([`Server`], [`serve_peer`]): `initialize` first (`-32002` otherwise), the [`Authorizer`]'s
//!   per-call [`Decision`], `ui.confirm`/`ui.prompt` confirmations sent back to the requester, then the
//!   [`Router`].
//! - **Transports** ([`Endpoint`]): a Unix socket in a 0700 directory with peer-uid checks on both ends, a
//!   named pipe with an explicit DACL (Windows), TCP with a token (WSL split mode), and [`in_process`].
//! - **Autostart** ([`connect_or_spawn`]): connect, or take `runtime/spawn.lock` and start the client's own
//!   executable as the daemon; development clients also restart an idle daemon of another build
//!   (ARCHITECTURE §3.3).

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use russet_core::error::{Error, Result};
use russet_core::ids::{JobId, LeaseId};
use russet_core::rpc::{ClientKind, ConnInfo, Ownership};
use russet_protocol::MethodInfo;
use russet_protocol::initialize::{ClientCapabilities, InitializeParams};

pub mod client;
pub mod codec;
pub mod message;
pub mod peer;
pub mod router;
pub mod server;
pub mod transport;

pub use client::{DevelopmentCheck, connect, connect_or_spawn, connect_with, initialize};
pub use message::{Message, RequestId, RpcError};
pub use peer::{BoxFuture, CallOptions, Handler, IncomingRequest, Peer, ProgressReporter, Role, Session};
pub use router::{ConnCtx, Layer, Next, RequestCtx, Router};
pub use russet_protocol::initialize::Grant;
pub use server::{Server, serve_peer};
pub use transport::Endpoint;

/// Frames are at most 16 MiB (ARCHITECTURE §7.2).
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// A `$/progress` value (ARCHITECTURE §7.6).
pub type ProgressValue = russet_protocol::common::WorkDoneProgress;

/// The client half of `initialize.params`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub kind: ClientKind,
    pub name: String,
    pub version: String,
    pub pid: u32,
    /// What this client answers (`ui.confirm`, `ui.prompt`, …) and whether it has a TTY.
    pub capabilities: ClientCapabilities,
    /// Absolute project roots.
    pub workspace_roots: Vec<String>,
    /// A device reservation to share (`RUSSET_LEASE`, ARCHITECTURE §6.3).
    pub shared_lease: Option<LeaseId>,
}

impl ClientInfo {
    /// A client of `kind` named `name`, with this build's version and this process's pid.
    pub fn new(kind: ClientKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
            version: russet_core::build_info::VERSION.to_owned(),
            pid: std::process::id(),
            capabilities: ClientCapabilities::default(),
            workspace_roots: Vec::new(),
            shared_lease: None,
        }
    }

    /// The `initialize` params, with `token` for TCP connections.
    pub fn initialize_params(&self, token: Option<&str>) -> InitializeParams {
        InitializeParams {
            protocol: russet_protocol::PROTOCOL,
            client: russet_protocol::initialize::ClientDescriptor {
                kind: self.kind,
                name: self.name.clone(),
                version: self.version.clone(),
                pid: Some(self.pid),
            },
            capabilities: self.capabilities,
            workspace: (!self.workspace_roots.is_empty())
                .then(|| russet_protocol::initialize::Workspace { roots: self.workspace_roots.clone() }),
            auth: token.map(|t| russet_protocol::initialize::AuthParams {
                token: russet_protocol::initialize::AuthToken(t.to_owned()),
            }),
            shared_lease: self.shared_lease.clone(),
        }
    }
}

/// How [`connect_or_spawn`] starts a daemon: its own executable as `daemon run --detach` (ARCHITECTURE §3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    /// How long to wait for the new daemon to accept connections (10 s).
    pub ready_timeout: Duration,
    /// Development builds: what the running daemon must match, or be restarted (ARCHITECTURE §3.3 item 3).
    pub expect: Option<DevelopmentCheck>,
}

/// What a confirmation asks (ARCHITECTURE §7.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmSpec {
    pub title: String,
    pub message: String,
    pub detail: Option<String>,
    pub danger: bool,
    /// A typed confirmation (`Confirm::Typed`): the text the user must type, or pass as `confirmText`.
    pub typed: Option<String>,
}

/// The authorizer's verdict for one call.
#[derive(Debug)]
pub enum Decision {
    Allow,
    /// Ask the requester first (`ui.confirm`, or `ui.prompt` for a typed confirmation).
    Confirm(ConfirmSpec),
    /// `perm.approvalRequired` or `perm.approvalInvalid`, with the approval's details (ARCHITECTURE §11.3).
    NeedsApproval(Error),
    /// `perm.denied`, `perm.tierExceeded`, `perm.confirmationUnavailable`, `device.liveBackendRefused`, ….
    Deny(Error),
}

/// Transport a peer connected over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Unix,
    NamedPipe,
    Tcp,
    InProcess,
}

/// Who is on the other end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIdentity {
    pub uid: Option<u32>,
    pub sid: Option<String>,
    pub pid: Option<u32>,
    pub transport: TransportKind,
}

/// Ownership queries that answer "no" (connections own nothing).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOwnership;

impl Ownership for NoOwnership {
    fn owns_job(&self, _conn: &str, _job: &JobId) -> bool {
        false
    }
    fn owns_lease(&self, _conn: &str, _lease: &LeaseId) -> bool {
        false
    }
    fn project_bundle_ids(&self, _conn: &str) -> Vec<String> {
        Vec::new()
    }
}

static NO_OWNERSHIP: LazyLock<NoOwnership> = LazyLock::new(NoOwnership::default);

/// Connection and call authorization; russetd implements it over its PermissionEngine.
pub trait Authorizer: Send + Sync {
    /// uid, SID and token checks.
    fn on_connect(&self, id: &PeerIdentity) -> Result<()>;
    fn on_initialize(&self, id: &PeerIdentity, p: &InitializeParams) -> Result<Grant>;
    /// The verdict for one call. The call's class is `(method.permission_of)(params, conn, ownership)`
    /// (ARCHITECTURE §7.5 [D 66]): the ceiling for static rows, [`Method::permission`] for the dynamic ones.
    ///
    /// [`Method::permission`]: russet_core::rpc::Method::permission
    fn decide(
        &self,
        grant: &Grant,
        method: &MethodInfo,
        params: &serde_json::Value,
        conn: &ConnInfo,
        ownership: &dyn Ownership,
    ) -> Decision;
    /// The ownership queries the server passes to [`Authorizer::decide`].
    fn ownership(&self) -> &dyn Ownership {
        &*NO_OWNERSHIP
    }
    /// A confirmation was answered; every confirmation is recorded as an approval granted by the requester
    /// (ARCHITECTURE §7.5).
    fn on_confirmation(&self, _conn: &ConnInfo, _method: &MethodInfo, _params: &serde_json::Value, _confirmed: bool) {}
}

/// Listener options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListenOptions {
    /// TCP only: accept non-loopback peers (`--allow-nonlocal`).
    pub allow_nonlocal: bool,
}

/// An in-process duplex `(client, server)`: `--no-daemon`, tests, and the MCP HTTP service inside the daemon.
/// The server half waits for [`serve_peer`] before it reads anything.
pub fn in_process() -> (Peer, Peer) {
    let (a, b) = tokio::io::duplex(1 << 16);
    let identity =
        PeerIdentity { uid: None, sid: None, pid: Some(std::process::id()), transport: TransportKind::InProcess };
    let client = Peer::start(a, Role::Client, identity.clone(), String::new());
    let server = Peer::start(b, Role::Server, identity, peer::next_connection_id());
    (client, server)
}

/// Wraps an `Arc<dyn Authorizer>` for code that holds one.
pub type SharedAuthorizer = Arc<dyn Authorizer>;
