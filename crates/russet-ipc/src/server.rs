// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Serving connections (ARCHITECTURE §7.4–§7.6): the per-connection pipeline and the listener.
//!
//! For every request on a server-side [`Peer`]:
//! 1. Before `initialize`, everything else gets `-32002`. `initialize` itself checks the TCP token, asks the
//!    [`Authorizer`] for the grant, records the [`Session`] and runs the router's `initialize` handler; if that
//!    fails (for example `ipc.protocolMismatch`), the error is sent and the connection closed.
//! 2. An unknown method gets `-32601`.
//! 3. The authorizer decides. `Deny` and `NeedsApproval` answer with their error (a dynamic row whose params
//!    do not deserialize answers `-32602`). `Confirm` sends `ui.confirm`, or `ui.prompt` with
//!    `typed-confirmation`, to the requester, unless a typed confirmation's `confirmText` already matches; a
//!    client that cannot answer gets `perm.confirmationUnavailable`, a refusal (or no answer within 120 s)
//!    `perm.confirmationDeclined`.
//! 4. The router (and its layer) runs the handler.

use std::sync::Arc;
use std::time::Duration;

use russet_core::error::{Error, Result, codes};
use russet_core::rpc::{ClientKind, ConnInfo};
use russet_protocol::common::ClientRef;
use russet_protocol::initialize::InitializeParams;
use russet_protocol::ui::{PromptKind, UiConfirm, UiConfirmParams, UiPrompt, UiPromptParams};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::message::{INVALID_PARAMS, INVALID_REQUEST, RpcError};
use crate::peer::{BoxFuture, Handler, IncomingRequest, Peer, ProgressReporter, Role, Session};
use crate::router::{ConnCtx, RequestCtx, Router, parse_params};
use crate::transport::{Accepted, Endpoint, Listener};
use crate::{Authorizer, CallOptions, ConfirmSpec, Decision, ListenOptions, TransportKind};

/// How long a confirmation may stay unanswered (core §7.5).
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a TCP connection has to authenticate (ARCHITECTURE §7.1).
pub const TCP_AUTH_DEADLINE: Duration = Duration::from_secs(2);

/// A bound listener.
pub struct Server {
    listener: Listener,
    tcp_token: Option<Arc<SecretString>>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").field("endpoint", &self.listener.describe()).finish()
    }
}

impl Server {
    /// Binds `endpoint`: a Unix socket 0600 in a 0700 directory (a stale socket from a crashed daemon is
    /// replaced, a live one refused), a named pipe with its DACL, or a TCP port.
    pub async fn bind(endpoint: &Endpoint, opts: ListenOptions) -> Result<Self> {
        let listener = Listener::bind(endpoint, opts.allow_nonlocal).await?;
        let tcp_token = match endpoint {
            Endpoint::Tcp { token, .. } => Some(Arc::new(SecretString::from(token.expose_secret().to_owned()))),
            #[cfg(any(unix, windows))]
            _ => None,
        };
        Ok(Self { listener, tcp_token })
    }

    /// The bound endpoint as a string (`unix:…`, `pipe:…`, `tcp://127.0.0.1:<port>`).
    pub fn endpoint(&self) -> String {
        self.listener.describe()
    }

    /// Accepts connections until `shutdown` fires; every connection runs the pipeline over `router`. Returns
    /// once the listener is closed (the socket file is removed); open connections stay up until they close.
    pub async fn serve<C: Clone + Send + Sync + 'static>(
        mut self,
        router: Arc<Router<C>>,
        ctx: C,
        auth: Arc<dyn Authorizer>,
        shutdown: CancellationToken,
    ) -> Result<()> {
        loop {
            let accepted = tokio::select! {
                () = shutdown.cancelled() => break,
                a = self.listener.accept() => a,
            };
            let accepted = match accepted {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!("accepting a connection failed: {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            let peer = match accepted {
                #[cfg(unix)]
                Accepted::Unix(stream, identity) => {
                    if let Err(e) = auth.on_connect(&identity) {
                        tracing::warn!("refused a connection from uid {:?}: {}", identity.uid, e.message());
                        continue;
                    }
                    Peer::start(stream, Role::Server, identity, crate::peer::next_connection_id())
                }
                Accepted::Tcp(stream, identity) => {
                    if let Err(e) = auth.on_connect(&identity) {
                        tracing::warn!("refused a TCP connection: {}", e.message());
                        continue;
                    }
                    Peer::start(stream, Role::Server, identity, crate::peer::next_connection_id())
                }
                #[cfg(windows)]
                Accepted::Pipe(pipe, identity) => {
                    if let Err(e) = auth.on_connect(&identity) {
                        tracing::warn!("refused a pipe connection: {}", e.message());
                        continue;
                    }
                    Peer::start(pipe, Role::Server, identity, crate::peer::next_connection_id())
                }
            };
            if peer.identity().transport == TransportKind::Tcp {
                let p = peer.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(TCP_AUTH_DEADLINE).await;
                    if p.session().is_none() {
                        p.close();
                    }
                });
            }
            install(&peer, Arc::clone(&router), ctx.clone(), Arc::clone(&auth), self.tcp_token.clone());
        }
        self.listener.cleanup();
        Ok(())
    }
}

/// Serves `peer` (the server half of [`crate::in_process`], or any server-role peer) with the pipeline.
pub fn serve_peer<C: Clone + Send + Sync + 'static>(
    peer: &Peer,
    router: Arc<Router<C>>,
    ctx: C,
    auth: Arc<dyn Authorizer>,
) {
    install(peer, router, ctx, auth, None);
}

fn install<C: Clone + Send + Sync + 'static>(
    peer: &Peer,
    router: Arc<Router<C>>,
    ctx: C,
    auth: Arc<dyn Authorizer>,
    tcp_token: Option<Arc<SecretString>>,
) {
    peer.set_handler(Arc::new(ServerHandler { router, ctx, auth, tcp_token }));
}

struct ServerHandler<C: Clone + Send + Sync + 'static> {
    router: Arc<Router<C>>,
    ctx: C,
    auth: Arc<dyn Authorizer>,
    tcp_token: Option<Arc<SecretString>>,
}

impl<C: Clone + Send + Sync + 'static> Handler for ServerHandler<C> {
    fn request(&self, peer: Peer, rq: IncomingRequest) -> BoxFuture<std::result::Result<Value, RpcError>> {
        let router = Arc::clone(&self.router);
        let ctx = self.ctx.clone();
        let auth = Arc::clone(&self.auth);
        let token = self.tcp_token.clone();
        Box::pin(async move { pipeline(router, ctx, auth, token, peer, rq).await })
    }

    fn notification(&self, peer: &Peer, method: String, params: Value) {
        if peer.session().is_none() {
            return;
        }
        let conn = ConnCtx { peer: peer.clone() };
        if let Some(fut) = self.router.dispatch_notification(self.ctx.clone(), conn, &method, params) {
            tokio::spawn(fut);
        }
    }
}

async fn pipeline<C: Clone + Send + Sync + 'static>(
    router: Arc<Router<C>>,
    ctx: C,
    auth: Arc<dyn Authorizer>,
    tcp_token: Option<Arc<SecretString>>,
    peer: Peer,
    rq: IncomingRequest,
) -> std::result::Result<Value, RpcError> {
    let Some(session) = peer.session().cloned() else {
        if rq.method != "initialize" {
            return Err(RpcError::not_initialized());
        }
        return initialize(router, ctx, auth, tcp_token, peer, rq).await;
    };
    if rq.method == "initialize" {
        return Err(RpcError::standard(
            INVALID_REQUEST,
            "the connection is already initialized",
            Error::invalid_argument("the connection is already initialized"),
        ));
    }
    let Some(info) = router.method_info(&rq.method) else {
        return Err(RpcError::method_not_found(&rq.method));
    };
    match auth.decide(&session.grant, &info, &rq.params, &session.conn, auth.ownership()) {
        Decision::Allow => {}
        Decision::Deny(e) if e.code() == codes::core::INVALID_ARGUMENT && info.dynamic_permission => {
            return Err(RpcError::standard(INVALID_PARAMS, e.message().to_owned(), e));
        }
        Decision::Deny(e) | Decision::NeedsApproval(e) => return Err(RpcError::from(&e)),
        Decision::Confirm(spec) => {
            let confirmed = confirm(&peer, &session, &spec, &rq.params, &rq.cancel).await?;
            auth.on_confirmation(&session.conn, &info, &rq.params, confirmed);
            if !confirmed {
                return Err(RpcError::from(&Error::new(
                    codes::perm::CONFIRMATION_DECLINED,
                    format!("{} was not confirmed", info.name),
                )));
            }
        }
    }
    let rq_ctx = RequestCtx {
        progress: ProgressReporter::for_request(&peer, &rq.params),
        peer,
        cancel: rq.cancel,
        request_id: rq.id,
    };
    match router.dispatch(ctx, rq_ctx, &rq.method, rq.params) {
        Some(f) => f.await,
        None => Err(RpcError::method_not_found(&rq.method)),
    }
}

/// Whether two secrets are equal, in time that depends only on their lengths.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn initialize<C: Clone + Send + Sync + 'static>(
    router: Arc<Router<C>>,
    ctx: C,
    auth: Arc<dyn Authorizer>,
    tcp_token: Option<Arc<SecretString>>,
    peer: Peer,
    rq: IncomingRequest,
) -> std::result::Result<Value, RpcError> {
    let params: InitializeParams = parse_params("initialize", rq.params.clone())?;
    if peer.identity().transport == TransportKind::Tcp {
        let presented = params.auth.as_ref().map(|a| a.token.0.as_bytes());
        let ok = match (&tcp_token, presented) {
            (Some(expected), Some(given)) => constant_time_eq(expected.expose_secret().as_bytes(), given),
            _ => false,
        };
        if !ok {
            peer.close_after_reply();
            return Err(RpcError::from(
                &Error::new(codes::perm::AUTH_FAILED, "the TCP token is missing or wrong")
                    .with_hint("Read the token from runtime/tcp.json of the daemon's home."),
            ));
        }
    }
    let grant = match auth.on_initialize(peer.identity(), &params) {
        Ok(g) => g,
        Err(e) => {
            peer.close_after_reply();
            return Err(RpcError::from(&e));
        }
    };
    let interactive = params.capabilities.interactive && params.client.kind == ClientKind::Cli;
    // The name is a display string that reaches logs, the audit log, job owners and `daemon.status`: anything
    // secret-shaped in it is masked once, here (ARCHITECTURE §12.2).
    let conn = ConnInfo {
        id: peer.connection_id().to_owned(),
        kind: params.client.kind,
        name: russet_core::redact::redact_text(&params.client.name).into_owned(),
        interactive,
        shared_lease: params.shared_lease.clone(),
    };
    peer.set_session(Session {
        conn,
        grant,
        capabilities: params.capabilities,
        client_version: params.client.version.clone(),
        client_pid: params.client.pid,
        workspace_roots: params.workspace.map(|w| w.roots).unwrap_or_default(),
    });
    let rq_ctx = RequestCtx {
        progress: ProgressReporter::for_request(&peer, &rq.params),
        peer: peer.clone(),
        cancel: rq.cancel,
        request_id: rq.id,
    };
    let result = match router.dispatch(ctx, rq_ctx, "initialize", rq.params) {
        Some(f) => f.await,
        None => Err(RpcError::internal("this server has no initialize handler")),
    };
    if result.is_err() {
        peer.close_after_reply();
    }
    result
}

/// Asks the requester to confirm (ARCHITECTURE §7.5). `Ok(false)` is a refusal; `Err` is a confirmation that
/// cannot happen (`perm.confirmationUnavailable`) or a cancelled request.
async fn confirm(
    peer: &Peer,
    session: &Session,
    spec: &ConfirmSpec,
    params: &Value,
    cancel: &CancellationToken,
) -> std::result::Result<bool, RpcError> {
    let requested_by = ClientRef { kind: session.conn.kind, name: session.conn.name.clone() };
    let timeout_ms = u64::try_from(CONFIRM_TIMEOUT.as_millis()).unwrap_or(u64::MAX);
    let unavailable = |why: &str| {
        RpcError::from(&Error::new(codes::perm::CONFIRMATION_UNAVAILABLE, format!("{}: {why}", spec.title)))
    };
    let opts = || CallOptions { cancel: Some(cancel.clone()), timeout: Some(CONFIRM_TIMEOUT), on_progress: None };
    let answered = |e: Error| -> std::result::Result<bool, RpcError> {
        if e.code() == codes::core::CANCELLED && cancel.is_cancelled() {
            Err(RpcError::cancelled())
        } else if e.code() == codes::core::NOT_FOUND {
            // The client does not implement the request.
            Err(unavailable("the client cannot answer confirmations"))
        } else {
            // A timeout or a broken client counts as declined.
            Ok(false)
        }
    };
    if let Some(expected) = &spec.typed {
        if params.get("confirmText").and_then(Value::as_str) == Some(expected.as_str()) {
            return Ok(true);
        }
        if !session.capabilities.prompt {
            return Err(unavailable("a typed confirmation needs a terminal or `--confirm-text`"));
        }
        let p = UiPromptParams {
            title: spec.title.clone(),
            message: spec.message.clone(),
            kind: PromptKind::TypedConfirmation,
            secret: false,
            expected: Some(expected.clone()),
            requested_by,
            timeout_ms,
        };
        return match peer.call_with::<UiPrompt>(p, opts()).await {
            Ok(r) => Ok(!r.cancelled && r.value.as_deref() == Some(expected.as_str())),
            Err(e) => answered(e),
        };
    }
    if !session.capabilities.confirm {
        return Err(unavailable("this client has no terminal; pass `--yes` to confirm"));
    }
    let p = UiConfirmParams {
        title: spec.title.clone(),
        message: spec.message.clone(),
        detail: spec.detail.clone(),
        danger: spec.danger,
        requested_by,
        timeout_ms,
    };
    match peer.call_with::<UiConfirm>(p, opts()).await {
        Ok(r) => Ok(r.confirmed),
        Err(e) => answered(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_comparison() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
