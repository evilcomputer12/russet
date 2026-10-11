// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The JSON-RPC engine (ARCHITECTURE §5.3 `russet-ipc`, §7; core §5.2): LSP codec, [`Peer`], [`Router`],
//! [`Authorizer`], Unix/pipe/TCP transports, the in-process duplex and the autostart client.
//!
//! F0 stub (W1); W2 owns this crate (CORE-04, CORE-05, freeze F1). [`Router`] already has a minimal working
//! body (registration, listing and dispatch of typed handlers) so the daemon's handler seam can be tested;
//! everything that touches a transport is `todo!()`.

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result, codes};
use russet_core::rpc::{ClientKind, ConnInfo, Method, NotificationMethod, Ownership, Permission};
use russet_protocol::initialize::{InitializeParams, InitializeResult};
use russet_protocol::{MethodInfo, RpcMethod, RpcNotification};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

/// Frames are at most 16 MiB (ARCHITECTURE §7.2).
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Where a daemon listens (ARCHITECTURE §7.1).
#[derive(Debug)]
pub enum Endpoint {
    #[cfg(unix)]
    Unix(PathBuf),
    #[cfg(windows)]
    NamedPipe(String),
    /// WSL split mode only (opt-in).
    Tcp { addr: SocketAddr, token: SecretString },
}

impl Endpoint {
    /// The default endpoint for `dirs` (`$RUSSET_HOME/runtime/russetd.sock`, …).
    pub fn default_for(dirs: &RussetDirs) -> Self {
        todo!("W2 (CORE-05)")
    }

    /// Parses `unix:/path`, `pipe:\\.\pipe\name` or `tcp://127.0.0.1:7777#<token-file>`.
    pub fn parse(s: &str) -> Result<Self> {
        todo!("W2 (CORE-05)")
    }
}

/// A JSON-RPC request id. Server-originated ids are strings `s:<n>` (ARCHITECTURE §7.3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RequestId {
    Num(i64),
    Str(String),
}

/// The client half of `initialize.params.client`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub kind: ClientKind,
    pub name: String,
    pub version: String,
    pub pid: u32,
}

/// How [`connect_or_spawn`] starts a daemon: its own executable as `daemon run --detach` (ARCHITECTURE §3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub ready_timeout: Duration,
}

/// One connection, usable from both ends.
#[derive(Debug, Clone)]
pub struct Peer {
    _f0: Arc<()>,
}

impl Peer {
    pub async fn call<M: Method>(&self, params: M::Params) -> Result<M::Result> {
        todo!("W2 (CORE-04)")
    }
    pub async fn call_with<M: Method>(&self, params: M::Params, opts: CallOptions) -> Result<M::Result> {
        todo!("W2 (CORE-04)")
    }
    pub fn notify<N: NotificationMethod>(&self, params: N::Params) -> Result<()> {
        todo!("W2 (CORE-04)")
    }
    /// The connection as the permission engine sees it.
    pub fn connection(&self) -> &ConnInfo {
        todo!("W2 (CORE-04)")
    }
    /// Resolves when the connection closes.
    pub async fn closed(&self) {
        todo!("W2 (CORE-04)")
    }
}

/// A `$/progress` value (ARCHITECTURE §7.6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgressValue {
    pub title: Option<String>,
    pub message: Option<String>,
    pub percentage: Option<u8>,
    pub cancellable: Option<bool>,
    pub job_id: Option<String>,
}

/// Per-call options.
#[derive(Default)]
pub struct CallOptions {
    /// Sends `$/cancelRequest` when fired.
    pub cancel: Option<CancellationToken>,
    /// Allocates a `progressToken`.
    pub on_progress: Option<Box<dyn FnMut(ProgressValue) + Send>>,
    pub timeout: Option<Duration>,
}

/// Reports `$/progress` for the request being handled.
#[derive(Debug, Clone)]
pub struct ProgressReporter {
    _f0: (),
}

/// What a request handler gets besides its context and params.
#[derive(Debug, Clone)]
pub struct RequestCtx {
    pub peer: Peer,
    pub cancel: CancellationToken,
    pub progress: ProgressReporter,
    pub request_id: RequestId,
}

/// What a notification handler gets besides its context and params.
#[derive(Debug, Clone)]
pub struct ConnCtx {
    pub peer: Peer,
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type RequestHandler<C> =
    Arc<dyn Fn(C, RequestCtx, serde_json::Value) -> BoxFuture<Result<serde_json::Value>> + Send + Sync>;
type NotificationHandler<C> = Arc<dyn Fn(C, ConnCtx, serde_json::Value) -> BoxFuture<()> + Send + Sync>;

/// Method name → typed handler.
pub struct Router<C: Clone + Send + Sync + 'static> {
    requests: BTreeMap<&'static str, (MethodInfo, RequestHandler<C>)>,
    notifications: BTreeMap<&'static str, NotificationHandler<C>>,
}

impl<C: Clone + Send + Sync + 'static> Router<C> {
    pub fn new() -> Self {
        Self { requests: BTreeMap::new(), notifications: BTreeMap::new() }
    }

    /// Registers the handler of method `M`. Registering a name twice is a programming error.
    ///
    /// Params that do not deserialize currently give `core.invalidArgument` (1006). W2 (freeze F1) maps them to
    /// JSON-RPC `-32602` with `details.path` at the IPC layer (ARCHITECTURE §13.1) and keeps 1006 for semantic
    /// validation inside handlers.
    pub fn request<M, F, Fut>(&mut self, handler: F) -> &mut Self
    where
        M: RpcMethod,
        F: Fn(C, RequestCtx, M::Params) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<M::Result>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let erased: RequestHandler<C> = Arc::new(move |cx, rq, params| {
            let handler = Arc::clone(&handler);
            Box::pin(async move {
                // TODO(W2, F1): -32602 with details.path instead of 1006 (ARCHITECTURE §13.1).
                let params: M::Params = serde_json::from_value(params).map_err(|e| {
                    Error::new(codes::core::INVALID_ARGUMENT, format!("invalid params for {}: {e}", M::NAME))
                })?;
                let result = handler(cx, rq, params).await?;
                serde_json::to_value(result).map_err(|e| {
                    Error::new(codes::core::INTERNAL, format!("cannot serialize the result of {}: {e}", M::NAME))
                })
            })
        });
        let previous = self.requests.insert(M::NAME, (M::INFO, erased));
        debug_assert!(previous.is_none(), "method {} registered twice", M::NAME);
        self
    }

    /// Registers the handler of notification `N`. Params that fail to parse are dropped.
    pub fn notification<N, F, Fut>(&mut self, handler: F) -> &mut Self
    where
        N: RpcNotification,
        F: Fn(C, ConnCtx, N::Params) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let erased: NotificationHandler<C> = Arc::new(move |cx, conn, params| {
            let handler = Arc::clone(&handler);
            Box::pin(async move {
                if let Ok(params) = serde_json::from_value::<N::Params>(params) {
                    handler(cx, conn, params).await;
                }
            })
        });
        let previous = self.notifications.insert(N::NAME, erased);
        debug_assert!(previous.is_none(), "notification {} registered twice", N::NAME);
        self
    }

    /// Every registered method (name, permission, long-running, schema hooks).
    pub fn methods(&self) -> impl Iterator<Item = MethodInfo> + '_ {
        self.requests.values().map(|(info, _)| *info)
    }

    /// Runs the handler of `method`; `None` when no handler is registered (JSON-RPC `-32601`).
    pub async fn dispatch(
        &self,
        cx: C,
        rq: RequestCtx,
        method: &str,
        params: serde_json::Value,
    ) -> Option<Result<serde_json::Value>> {
        let (_, handler) = self.requests.get(method)?;
        Some(handler(cx, rq, params).await)
    }
}

impl<C: Clone + Send + Sync + 'static> Default for Router<C> {
    fn default() -> Self {
        Self::new()
    }
}

/// The connection's grant (ARCHITECTURE §7.4 `grant`, §7.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grant {
    pub profile: String,
    pub allow: Vec<Permission>,
    pub confirm: Vec<Permission>,
    pub approval: Vec<Permission>,
    pub deny: Vec<Permission>,
    /// MCP connections only.
    pub tier_max: Option<u8>,
}

/// The authorizer's verdict for one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Confirm { title: String, message: String, danger: bool },
    NeedsApproval { tier: u8, summary: String },
    Deny { reason: String },
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

/// Connection and call authorization; russetd implements it over its PermissionEngine.
pub trait Authorizer: Send + Sync {
    /// uid, SID and token checks.
    fn on_connect(&self, id: &PeerIdentity) -> Result<()>;
    fn on_initialize(&self, id: &PeerIdentity, p: &InitializeParams) -> Result<Grant>;
    /// The verdict for one call. The call's class is `(method.permission_of)(params, conn, ownership)`
    /// (ARCHITECTURE §7.5 [D 66]): the ceiling for static rows, [`Method::permission`] for the dynamic ones.
    fn decide(
        &self,
        grant: &Grant,
        method: &MethodInfo,
        params: &serde_json::Value,
        conn: &ConnInfo,
        ownership: &dyn Ownership,
    ) -> Decision;
}

/// Listener options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListenOptions {
    /// TCP only: accept non-loopback peers (`--allow-nonlocal`).
    pub allow_nonlocal: bool,
}

/// A bound listener.
#[derive(Debug)]
pub struct Server {
    _f0: (),
}

impl Server {
    pub async fn bind(endpoint: &Endpoint, opts: ListenOptions) -> Result<Self> {
        todo!("W2 (CORE-05)")
    }

    pub async fn serve<C: Clone + Send + Sync + 'static>(
        self,
        router: Arc<Router<C>>,
        ctx: C,
        auth: Arc<dyn Authorizer>,
        shutdown: CancellationToken,
    ) -> Result<()> {
        todo!("W2 (CORE-05)")
    }
}

/// Connects and performs `initialize`.
pub async fn connect(endpoint: &Endpoint, client: ClientInfo) -> Result<(Peer, InitializeResult)> {
    todo!("W2 (CORE-05)")
}

/// Connects, spawning the daemon first when needed (ARCHITECTURE §3.3 item 1).
pub async fn connect_or_spawn(
    dirs: &RussetDirs,
    client: ClientInfo,
    spawn: SpawnSpec,
) -> Result<(Peer, InitializeResult)> {
    todo!("W2 (CORE-05)")
}

/// An in-process duplex: `--no-daemon`, tests, and the MCP HTTP service inside the daemon.
pub fn in_process() -> (Peer, Peer) {
    todo!("W2 (CORE-04)")
}

#[cfg(test)]
mod tests {
    use russet_protocol::daemon::{DaemonPing, DaemonStatus};
    use russet_protocol::{Method, Unspecified};

    use super::*;

    fn rq() -> RequestCtx {
        RequestCtx {
            peer: Peer { _f0: Arc::new(()) },
            cancel: CancellationToken::new(),
            progress: ProgressReporter { _f0: () },
            request_id: RequestId::Num(1),
        }
    }

    async fn ping(_cx: (), _rq: RequestCtx, _p: Unspecified) -> Result<Unspecified> {
        Ok(Unspecified {})
    }

    #[test]
    fn router_registers_lists_and_dispatches() {
        let mut router = Router::<()>::new();
        router.request::<DaemonPing, _, _>(ping);
        let names: Vec<&str> = router.methods().map(|m| m.name).collect();
        assert_eq!(names, vec![DaemonPing::NAME]);

        let rt = tokio::runtime::Builder::new_current_thread().build().expect("runtime");
        let ok = rt.block_on(router.dispatch((), rq(), "daemon.ping", serde_json::json!({})));
        assert_eq!(ok.map(|r| r.map_err(|e| e.code())), Some(Ok(serde_json::json!({}))));
        let missing = rt.block_on(router.dispatch((), rq(), DaemonStatus::NAME, serde_json::json!({})));
        assert!(missing.is_none());
        // F0 behaviour; W2 (F1) answers JSON-RPC -32602 for these at the IPC layer (ARCHITECTURE §13.1).
        let bad = rt.block_on(router.dispatch((), rq(), "daemon.ping", serde_json::json!(7)));
        assert_eq!(bad.map(|r| r.map_err(|e| e.code())), Some(Err(codes::core::INVALID_ARGUMENT)));
    }
}
