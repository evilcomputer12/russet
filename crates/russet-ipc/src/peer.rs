// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`Peer`]: one JSON-RPC connection, usable from both ends (core §5.2).
//!
//! A connection has a writer task (frames from an unbounded queue, so [`Peer::notify`] never blocks) and a
//! reader task. Incoming requests run in their own tasks, so a slow handler never stalls the connection, and a
//! panicking handler answers `-32603` instead of taking the connection down. `$/cancelRequest` cancels the
//! handler's token and answers `-32800` at once; `$/progress` reaches the `on_progress` callback of the call
//! that allocated the token. Requests the server sends to a client use string ids `s:<n>` (ARCHITECTURE §7.3).

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, RwLock};
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use russet_core::error::{Error, codes};
use russet_core::ids::JobId;
use russet_core::progress::{EventSink, JobEventKind, LogStream, ProgressSink};
use russet_core::rpc::{ClientKind, ConnInfo, Method, NotificationMethod};
use russet_protocol::common::{
    CancelParams, ProgressKind, ProgressParams, ProgressToken, RequestIdValue, WorkDoneProgress,
};
use russet_protocol::initialize::{ClientCapabilities, Grant};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use crate::codec::{CodecError, LspCodec};
use crate::message::{Message, RequestId, RpcError};
use crate::{PeerIdentity, ProgressValue};

/// A boxed, sendable future.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// How long a cancelled call waits for the `-32800` reply before giving up locally.
const CANCEL_REPLY_WAIT: Duration = Duration::from_secs(5);

/// Which end of the connection this peer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Sends `initialize`; numbers its requests 1, 2, 3, ….
    Client,
    /// Answers `initialize`; numbers its requests `s:1`, `s:2`, ….
    Server,
}

/// A request that arrived on a connection.
#[derive(Debug)]
pub struct IncomingRequest {
    pub id: RequestId,
    pub method: String,
    pub params: Value,
    /// Fired by `$/cancelRequest` and when the connection closes.
    pub cancel: CancellationToken,
}

/// Serves what arrives on a connection: the daemon's pipeline on the server side, the registered callbacks
/// (`ui.*`, notifications) on the client side.
pub trait Handler: Send + Sync + 'static {
    /// Answers a request.
    fn request(&self, peer: Peer, rq: IncomingRequest) -> BoxFuture<Result<Value, RpcError>>;
    /// Takes a notification (other than `$/progress` and `$/cancelRequest`, which the peer handles).
    fn notification(&self, peer: &Peer, method: String, params: Value);
}

/// What a connection knows after `initialize` (both ends).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The connection as the permission engine sees it.
    pub conn: ConnInfo,
    /// The connection's permissions.
    pub grant: Grant,
    /// What the client said it can answer.
    pub capabilities: ClientCapabilities,
    pub client_version: String,
    pub client_pid: Option<u32>,
    pub workspace_roots: Vec<String>,
}

/// Per-call options.
#[derive(Default)]
pub struct CallOptions {
    /// Sends `$/cancelRequest` when fired.
    pub cancel: Option<CancellationToken>,
    /// Allocates a `progressToken` and receives its `$/progress` values.
    pub on_progress: Option<Box<dyn FnMut(ProgressValue) + Send>>,
    /// Sends `$/cancelRequest` and fails with `core.timeout` after this long.
    pub timeout: Option<Duration>,
}

impl fmt::Debug for CallOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallOptions")
            .field("cancel", &self.cancel.is_some())
            .field("on_progress", &self.on_progress.is_some())
            .field("timeout", &self.timeout)
            .finish()
    }
}

enum Outgoing {
    Frame(Bytes),
    /// Flush everything queued before, then close the connection.
    Close,
}

type ProgressCallback = Box<dyn FnMut(ProgressValue) + Send>;
type ClientRequestFn = Arc<dyn Fn(Value) -> BoxFuture<Result<Value, RpcError>> + Send + Sync>;
type NotificationFn = Arc<dyn Fn(&str, &Value) + Send + Sync>;

struct PeerInner {
    role: Role,
    identity: PeerIdentity,
    /// Server side: assigned at accept. Client side: the `connectionId` of the initialize result.
    conn_id: String,
    out: mpsc::UnboundedSender<Outgoing>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<RequestId, oneshot::Sender<Result<Value, RpcError>>>>,
    next_token: AtomicU64,
    progress: Mutex<HashMap<String, ProgressCallback>>,
    incoming: Mutex<HashMap<RequestId, CancellationToken>>,
    handler: OnceLock<Arc<dyn Handler>>,
    handler_set: Notify,
    session: OnceLock<Session>,
    closed: CancellationToken,
    /// Close once the reply being handled is queued (a failed `initialize`).
    close_after_reply: AtomicBool,
    /// Frames queued for the writer and not yet written (subscriptions use it to bound slow consumers).
    backlog: Arc<AtomicUsize>,
    client: Option<Arc<ClientHandler>>,
}

/// One connection, usable from both ends.
#[derive(Clone)]
pub struct Peer {
    inner: Arc<PeerInner>,
}

impl fmt::Debug for Peer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Peer")
            .field("role", &self.inner.role)
            .field("conn", &self.connection().id)
            .field("closed", &self.is_closed())
            .finish()
    }
}

static UNINITIALIZED: LazyLock<ConnInfo> = LazyLock::new(|| ConnInfo {
    id: String::new(),
    kind: ClientKind::Test,
    name: String::new(),
    interactive: false,
    shared_lease: None,
});

/// A fresh connection id `c-<n>` (ARCHITECTURE §7.3).
pub(crate) fn next_connection_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("c-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

impl Peer {
    /// Starts a connection over `io`. A client-role peer gets the callback handler of
    /// [`Peer::on_request`]/[`Peer::on_notification`]; a server-role peer waits for [`Peer::set_handler`]
    /// before it reads the first message.
    pub fn start<T>(io: T, role: Role, identity: PeerIdentity, conn_id: String) -> Peer
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (read, write) = tokio::io::split(io);
        let (tx, rx) = mpsc::unbounded_channel();
        let client = (role == Role::Client).then(|| Arc::new(ClientHandler::default()));
        let backlog = Arc::new(AtomicUsize::new(0));
        let peer = Peer {
            inner: Arc::new(PeerInner {
                role,
                identity,
                conn_id,
                out: tx,
                next_id: AtomicI64::new(1),
                pending: Mutex::new(HashMap::new()),
                next_token: AtomicU64::new(1),
                progress: Mutex::new(HashMap::new()),
                incoming: Mutex::new(HashMap::new()),
                handler: OnceLock::new(),
                handler_set: Notify::new(),
                session: OnceLock::new(),
                closed: CancellationToken::new(),
                close_after_reply: AtomicBool::new(false),
                backlog: Arc::clone(&backlog),
                client: client.clone(),
            }),
        };
        if let Some(client) = client {
            peer.set_handler(client);
        }
        tokio::spawn(write_loop(FramedWrite::new(write, LspCodec::default()), rx, peer.inner.closed.clone(), backlog));
        tokio::spawn(read_loop(peer.clone(), FramedRead::new(read, LspCodec::default())));
        peer
    }

    /// Installs the handler of incoming messages (once; later calls are ignored).
    pub fn set_handler(&self, handler: Arc<dyn Handler>) {
        if self.inner.handler.set(handler).is_ok() {
            self.inner.handler_set.notify_waiters();
            self.inner.handler_set.notify_one();
        }
    }

    /// Which end this peer is.
    pub fn role(&self) -> Role {
        self.inner.role
    }

    /// Who is on the other end.
    pub fn identity(&self) -> &PeerIdentity {
        &self.inner.identity
    }

    /// The connection as the permission engine sees it (an empty id before `initialize`).
    pub fn connection(&self) -> &ConnInfo {
        self.inner.session.get().map_or(&UNINITIALIZED, |s| &s.conn)
    }

    /// The connection id (`c-<n>`), known at accept on the server side.
    pub fn connection_id(&self) -> &str {
        match self.inner.session.get() {
            Some(s) => &s.conn.id,
            None => &self.inner.conn_id,
        }
    }

    /// The session, after `initialize`.
    pub fn session(&self) -> Option<&Session> {
        self.inner.session.get()
    }

    /// Records the session (once). Servers call it while handling `initialize`; clients after it.
    pub fn set_session(&self, s: Session) -> bool {
        self.inner.session.set(s).is_ok()
    }

    /// Whether the connection is closed.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.is_cancelled()
    }

    /// Resolves when the connection closes.
    pub async fn closed(&self) {
        self.inner.closed.cancelled().await;
    }

    /// A token that fires when the connection closes.
    pub fn closed_token(&self) -> CancellationToken {
        self.inner.closed.clone()
    }

    /// Flushes what is queued, then closes the connection.
    pub fn close(&self) {
        let _ = self.inner.out.send(Outgoing::Close);
    }

    /// Closes the connection right after the reply to the request being handled (a failed `initialize`).
    pub fn close_after_reply(&self) {
        self.inner.close_after_reply.store(true, Ordering::SeqCst);
    }

    fn send(&self, m: &Message) -> russet_core::Result<()> {
        let body = m.to_bytes();
        if body.len() > crate::MAX_FRAME_BYTES {
            return Err(Error::new(
                codes::ipc::FRAME_TOO_LARGE,
                format!("a {} byte message is larger than the 16 MiB frame limit", body.len()),
            ));
        }
        self.inner.backlog.fetch_add(1, Ordering::Relaxed);
        self.inner.out.send(Outgoing::Frame(Bytes::from(body))).map_err(|_| {
            self.inner.backlog.fetch_sub(1, Ordering::Relaxed);
            Error::new(codes::ipc::CONNECTION_CLOSED, "the connection is closed")
        })
    }

    /// Frames queued for this connection and not yet written: a slow reader makes it grow, and subscriptions
    /// hold back (dropping their oldest items) while it is high (ARCHITECTURE §6.4).
    pub fn backlog(&self) -> usize {
        self.inner.backlog.load(Ordering::Relaxed)
    }

    /// Sends a response (used by the read loop; responses that are too large become `ipc.frameTooLarge`).
    fn respond(&self, id: Option<RequestId>, result: Result<Value, RpcError>) {
        let m = Message::Response { id: id.clone(), result };
        if let Err(e) = self.send(&m)
            && e.code() == codes::ipc::FRAME_TOO_LARGE
        {
            let _ = self.send(&Message::Response { id, result: Err(RpcError::from(&e)) });
        }
    }

    /// Calls method `M` and waits for its result.
    pub async fn call<M: Method>(&self, params: M::Params) -> russet_core::Result<M::Result> {
        self.call_with::<M>(params, CallOptions::default()).await
    }

    /// Calls method `M` with cancellation, progress and a timeout.
    pub async fn call_with<M: Method>(&self, params: M::Params, opts: CallOptions) -> russet_core::Result<M::Result> {
        let params = serde_json::to_value(params)
            .map_err(|e| Error::internal(format!("cannot serialize the params of {}: {e}", M::NAME)))?;
        let value = self.call_raw(M::NAME, params, opts).await.map_err(|e| e.to_error())?;
        serde_json::from_value(value)
            .map_err(|e| Error::internal(format!("the result of {} does not have the expected shape: {e}", M::NAME)))
    }

    /// Calls `method` with raw JSON params; the error keeps its JSON-RPC code.
    pub async fn call_raw(&self, method: &str, mut params: Value, opts: CallOptions) -> Result<Value, RpcError> {
        if self.is_closed() {
            return Err(RpcError::from(&Error::new(codes::ipc::CONNECTION_CLOSED, "the connection is closed")));
        }
        let token = match opts.on_progress {
            Some(cb) => {
                let token = format!("p-{}", self.inner.next_token.fetch_add(1, Ordering::Relaxed));
                if let Value::Object(m) = &mut params {
                    m.insert("progressToken".into(), Value::String(token.clone()));
                }
                lock(&self.inner.progress).insert(token.clone(), cb);
                Some(token)
            }
            None => None,
        };
        let id = match self.inner.role {
            Role::Client => RequestId::Num(self.inner.next_id.fetch_add(1, Ordering::Relaxed)),
            Role::Server => RequestId::Str(format!("s:{}", self.inner.next_id.fetch_add(1, Ordering::Relaxed))),
        };
        let (tx, mut rx) = oneshot::channel();
        lock(&self.inner.pending).insert(id.clone(), tx);
        let sent = self.send(&Message::Request { id: id.clone(), method: method.to_owned(), params });
        let result = match sent {
            Err(e) => Err(RpcError::from(&e)),
            Ok(()) => {
                let cancel = opts.cancel.unwrap_or_default();
                let timeout = async {
                    match opts.timeout {
                        Some(d) => tokio::time::sleep(d).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    r = &mut rx => r.unwrap_or_else(|_| Err(closed_error())),
                    () = cancel.cancelled() => {
                        self.cancel_request(&id);
                        match tokio::time::timeout(CANCEL_REPLY_WAIT, &mut rx).await {
                            Ok(Ok(r)) => r,
                            _ => Err(RpcError::cancelled()),
                        }
                    }
                    () = timeout => {
                        self.cancel_request(&id);
                        Err(RpcError::from(&Error::new(codes::core::TIMEOUT, format!("{method} timed out"))))
                    }
                }
            }
        };
        lock(&self.inner.pending).remove(&id);
        if let Some(token) = token {
            lock(&self.inner.progress).remove(&token);
        }
        result
    }

    fn cancel_request(&self, id: &RequestId) {
        let id = match id {
            RequestId::Num(n) => RequestIdValue::Number(*n),
            RequestId::Str(s) => RequestIdValue::String(s.clone()),
        };
        if let Ok(params) = serde_json::to_value(CancelParams { id }) {
            let _ = self.send(&Message::Notification { method: "$/cancelRequest".into(), params });
        }
    }

    /// Sends notification `N`.
    pub fn notify<N: NotificationMethod>(&self, params: N::Params) -> russet_core::Result<()> {
        let params = serde_json::to_value(params)
            .map_err(|e| Error::internal(format!("cannot serialize the params of {}: {e}", N::NAME)))?;
        self.notify_raw(N::NAME, params)
    }

    /// Sends a notification with raw JSON params.
    pub fn notify_raw(&self, method: &str, params: Value) -> russet_core::Result<()> {
        self.send(&Message::Notification { method: method.to_owned(), params })
    }

    /// Client side: answers server-to-client requests of method `M` (`ui.confirm`, `ui.prompt`, …).
    pub fn on_request<M, F, Fut>(&self, f: F)
    where
        M: Method,
        F: Fn(M::Params) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = russet_core::Result<M::Result>> + Send + 'static,
    {
        let Some(client) = &self.inner.client else { return };
        let f = Arc::new(f);
        let erased: ClientRequestFn = Arc::new(move |params| {
            let f = Arc::clone(&f);
            Box::pin(async move {
                let params: M::Params = crate::router::parse_params(M::NAME, params)?;
                let result = f(params).await.map_err(RpcError::from)?;
                serde_json::to_value(result)
                    .map_err(|e| RpcError::internal(format!("cannot serialize the result of {}: {e}", M::NAME)))
            })
        });
        write_lock(&client.requests).insert(M::NAME.to_owned(), erased);
    }

    /// Client side: calls `f` for every notification `N` (params that do not parse are skipped).
    pub fn on_notification<N, F>(&self, f: F)
    where
        N: NotificationMethod,
        F: Fn(N::Params) + Send + Sync + 'static,
    {
        self.on_notification_raw(N::NAME, move |_method, params| {
            if let Ok(p) = serde_json::from_value::<N::Params>(params.clone()) {
                f(p);
            }
        });
    }

    /// Client side: calls `f` for every notification named `method`, or for every notification when `method`
    /// is `"*"`.
    pub fn on_notification_raw<F>(&self, method: &str, f: F)
    where
        F: Fn(&str, &Value) + Send + Sync + 'static,
    {
        let Some(client) = &self.inner.client else { return };
        write_lock(&client.notifications).entry(method.to_owned()).or_default().push(Arc::new(f));
    }

    async fn wait_for_handler(&self) -> Option<Arc<dyn Handler>> {
        loop {
            if let Some(h) = self.inner.handler.get() {
                return Some(Arc::clone(h));
            }
            let notified = self.inner.handler_set.notified();
            if let Some(h) = self.inner.handler.get() {
                return Some(Arc::clone(h));
            }
            tokio::select! {
                () = notified => {}
                () = self.inner.closed.cancelled() => return None,
            }
        }
    }

    fn on_frame(&self, handler: &Arc<dyn Handler>, body: &[u8]) {
        match Message::parse(body) {
            Err((id, err)) => self.respond(id, Err(err)),
            Ok(Message::Request { id, method, params }) => {
                let cancel = self.inner.closed.child_token();
                lock(&self.inner.incoming).insert(id.clone(), cancel.clone());
                let peer = self.clone();
                let handler = Arc::clone(handler);
                tokio::spawn(async move {
                    let rq = IncomingRequest { id: id.clone(), method, params, cancel: cancel.clone() };
                    let task = tokio::spawn(handler.request(peer.clone(), rq));
                    let result = tokio::select! {
                        joined = task => match joined {
                            Ok(r) => r,
                            Err(e) if e.is_panic() => Err(RpcError::internal("the request handler panicked")),
                            Err(_) => Err(RpcError::cancelled()),
                        },
                        () = cancel.cancelled() => Err(RpcError::cancelled()),
                    };
                    lock(&peer.inner.incoming).remove(&id);
                    if !peer.is_closed() {
                        peer.respond(Some(id), result);
                        if peer.inner.close_after_reply.load(Ordering::SeqCst) {
                            peer.close();
                        }
                    }
                });
            }
            Ok(Message::Notification { method, params }) => match method.as_str() {
                "$/cancelRequest" => {
                    if let Ok(p) = serde_json::from_value::<CancelParams>(params) {
                        let id = match p.id {
                            RequestIdValue::Number(n) => RequestId::Num(n),
                            RequestIdValue::String(s) => RequestId::Str(s),
                        };
                        if let Some(token) = lock(&self.inner.incoming).get(&id) {
                            token.cancel();
                        }
                    }
                }
                "$/progress" => {
                    if let Ok(p) = serde_json::from_value::<ProgressParams>(params) {
                        let key = match &p.token {
                            ProgressToken::String(s) => s.clone(),
                            ProgressToken::Number(n) => n.to_string(),
                        };
                        if let Some(cb) = lock(&self.inner.progress).get_mut(&key) {
                            cb(p.value);
                        }
                    }
                }
                _ => handler.notification(self, method, params),
            },
            Ok(Message::Response { id: Some(id), result }) => {
                if let Some(tx) = lock(&self.inner.pending).remove(&id) {
                    let _ = tx.send(result);
                }
            }
            Ok(Message::Response { id: None, result }) => {
                tracing::debug!(conn = %self.connection_id(), "the peer reported an error without an id: {result:?}");
            }
        }
    }

    fn shut_down(&self) {
        self.inner.closed.cancel();
        let _ = self.inner.out.send(Outgoing::Close);
        // Callers waiting for responses get ipc.connectionClosed (their senders drop).
        lock(&self.inner.pending).clear();
        lock(&self.inner.progress).clear();
    }
}

fn closed_error() -> RpcError {
    RpcError::from(&Error::new(codes::ipc::CONNECTION_CLOSED, "the connection closed before the reply"))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_lock<T>(m: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read_lock<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn write_loop<W: AsyncWrite + Unpin + Send + 'static>(
    mut sink: FramedWrite<W, LspCodec>,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    closed: CancellationToken,
    backlog: Arc<AtomicUsize>,
) {
    while let Some(item) = rx.recv().await {
        match item {
            Outgoing::Frame(body) => {
                let sent = sink.send(body).await;
                backlog.fetch_sub(1, Ordering::Relaxed);
                if let Err(e) = sent {
                    tracing::debug!("connection write failed: {e}");
                    break;
                }
            }
            Outgoing::Close => break,
        }
    }
    let _ = sink.close().await;
    closed.cancel();
}

async fn read_loop<R: AsyncRead + Unpin + Send + 'static>(peer: Peer, mut stream: FramedRead<R, LspCodec>) {
    // A server-side peer may get its handler after it started; frames wait for it, an end of stream does not.
    let mut handler: Option<Arc<dyn Handler>> = None;
    loop {
        tokio::select! {
            () = peer.inner.closed.cancelled() => break,
            frame = stream.next() => match frame {
                None => break,
                Some(Ok(body)) => {
                    let h = match &handler {
                        Some(h) => Arc::clone(h),
                        None => match peer.wait_for_handler().await {
                            Some(h) => {
                                handler = Some(Arc::clone(&h));
                                h
                            }
                            None => break,
                        },
                    };
                    peer.on_frame(&h, &body);
                }
                Some(Err(CodecError::FrameTooLarge(n))) => {
                    let e = Error::new(
                        codes::ipc::FRAME_TOO_LARGE,
                        format!("a frame of {n} bytes is larger than the 16 MiB limit; closing the connection"),
                    );
                    peer.respond(None, Err(RpcError::from(&e)));
                    break;
                }
                Some(Err(e)) => {
                    if !matches!(e, CodecError::Io(_)) {
                        let err = Error::invalid_argument(format!("{e}; closing the connection"));
                        peer.respond(None, Err(RpcError::standard(crate::message::PARSE_ERROR, e.to_string(), err)));
                    }
                    break;
                }
            }
        }
    }
    peer.shut_down();
}

/// The client side's callbacks.
#[derive(Default)]
struct ClientHandler {
    requests: RwLock<HashMap<String, ClientRequestFn>>,
    notifications: RwLock<HashMap<String, Vec<NotificationFn>>>,
}

impl Handler for ClientHandler {
    fn request(&self, _peer: Peer, rq: IncomingRequest) -> BoxFuture<Result<Value, RpcError>> {
        let f = read_lock(&self.requests).get(&rq.method).cloned();
        match f {
            Some(f) => f(rq.params),
            None => Box::pin(async move { Err(RpcError::method_not_found(&rq.method)) }),
        }
    }

    fn notification(&self, _peer: &Peer, method: String, params: Value) {
        let callbacks: Vec<NotificationFn> = {
            let map = read_lock(&self.notifications);
            map.get(&method).into_iter().chain(map.get("*")).flatten().cloned().collect()
        };
        for cb in callbacks {
            cb(&method, &params);
        }
    }
}

/// Reports `$/progress` for the request being handled (ARCHITECTURE §7.6), and forwards progress, log lines
/// and job events to an optional tap (the daemon's job event log). It implements [`EventSink`], so handlers
/// pass it to domain APIs as is.
#[derive(Clone, Default)]
pub struct ProgressReporter {
    peer: Option<Peer>,
    token: Option<ProgressToken>,
    job_id: Option<JobId>,
    tap: Option<Arc<dyn EventSink>>,
    /// The `begin` was already sent (by the daemon's job layer): later `begin`s become `report`s.
    begun: bool,
}

impl fmt::Debug for ProgressReporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProgressReporter")
            .field("token", &self.token)
            .field("job_id", &self.job_id)
            .field("tap", &self.tap.is_some())
            .field("begun", &self.begun)
            .finish()
    }
}

impl ProgressReporter {
    /// A reporter for a request whose params may carry `progressToken`.
    pub fn for_request(peer: &Peer, params: &Value) -> Self {
        let token = params.get("progressToken").and_then(|t| serde_json::from_value::<ProgressToken>(t.clone()).ok());
        Self { peer: Some(peer.clone()), token, job_id: None, tap: None, begun: false }
    }

    /// A reporter that drops everything.
    pub fn none() -> Self {
        Self::default()
    }

    /// This reporter, also forwarding everything to `tap`.
    pub fn with_tap(mut self, tap: Arc<dyn EventSink>) -> Self {
        self.tap = Some(tap);
        self
    }

    /// This reporter, naming `job` in its `begin` value.
    pub fn with_job(mut self, job: JobId) -> Self {
        self.job_id = Some(job);
        self
    }

    /// This reporter after its `begin` was sent: a handler's own `begin` becomes a `report` with the title as
    /// the message, so the requester sees one `begin` per token (LSP work-done progress).
    pub fn begun(mut self) -> Self {
        self.begun = true;
        self
    }

    /// Whether the requester asked for progress.
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// Sends one `$/progress` value (when the requester asked for progress).
    pub fn send(&self, mut value: WorkDoneProgress) {
        if value.kind == ProgressKind::Begin && value.job_id.is_none() {
            value.job_id.clone_from(&self.job_id);
        }
        if let (Some(peer), Some(token)) = (&self.peer, &self.token)
            && let Ok(params) = serde_json::to_value(ProgressParams { token: token.clone(), value })
        {
            let _ = peer.notify_raw("$/progress", params);
        }
    }
}

impl ProgressSink for ProgressReporter {
    fn begin(&self, title: &str, cancellable: bool) {
        if self.begun {
            self.send(WorkDoneProgress::report(Some(title.to_owned()), None));
        } else {
            self.send(WorkDoneProgress::begin(title, cancellable));
        }
        if let Some(tap) = &self.tap {
            tap.begin(title, cancellable);
        }
    }

    fn report(&self, msg: Option<&str>, pct: Option<u8>) {
        self.send(WorkDoneProgress::report(msg.map(str::to_owned), pct.map(|p| p.min(100))));
        if let Some(tap) = &self.tap {
            tap.report(msg, pct);
        }
    }

    fn end(&self, msg: Option<&str>) {
        self.send(WorkDoneProgress::end(msg.map(str::to_owned)));
        if let Some(tap) = &self.tap {
            tap.end(msg);
        }
    }

    fn log(&self, stream: LogStream, line: &str) {
        if let Some(tap) = &self.tap {
            tap.log(stream, line);
        }
    }
}

impl EventSink for ProgressReporter {
    fn emit(&self, e: JobEventKind) {
        if let Some(tap) = &self.tap {
            tap.emit(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TransportKind;

    fn identity() -> PeerIdentity {
        PeerIdentity { uid: None, sid: None, pid: None, transport: TransportKind::InProcess }
    }

    /// A server-side handler that echoes params and sleeps on `slow`.
    struct Echo;

    impl Handler for Echo {
        fn request(&self, _peer: Peer, rq: IncomingRequest) -> BoxFuture<Result<Value, RpcError>> {
            Box::pin(async move {
                match rq.method.as_str() {
                    "slow" => {
                        rq.cancel.cancelled().await;
                        Err(RpcError::cancelled())
                    }
                    "panic" => {
                        #[allow(clippy::panic)]
                        if std::hint::black_box(true) {
                            panic!("expected test panic");
                        }
                        Ok(Value::Null)
                    }
                    _ => Ok(rq.params),
                }
            })
        }
        fn notification(&self, _peer: &Peer, _method: String, _params: Value) {}
    }

    fn pair() -> (Peer, Peer) {
        let (a, b) = tokio::io::duplex(1 << 16);
        let client = Peer::start(a, Role::Client, identity(), String::new());
        let server = Peer::start(b, Role::Server, identity(), next_connection_id());
        server.set_handler(Arc::new(Echo));
        (client, server)
    }

    #[tokio::test]
    async fn calls_round_trip_and_cancel() {
        let (client, _server) = pair();
        let v = client.call_raw("echo", serde_json::json!({"a": 1}), CallOptions::default()).await.expect("echo");
        assert_eq!(v, serde_json::json!({"a": 1}));

        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            c2.cancel();
        });
        let e = client
            .call_raw("slow", serde_json::json!({}), CallOptions { cancel: Some(cancel), ..CallOptions::default() })
            .await
            .expect_err("cancelled");
        assert_eq!(e.code, crate::message::REQUEST_CANCELLED);

        let e = client.call_raw("panic", serde_json::json!({}), CallOptions::default()).await.expect_err("panics");
        assert_eq!(e.code, crate::message::INTERNAL_ERROR);

        let e = client
            .call_raw(
                "slow",
                serde_json::json!({}),
                CallOptions { timeout: Some(Duration::from_millis(50)), ..CallOptions::default() },
            )
            .await
            .expect_err("times out");
        assert_eq!(e.to_error().code(), codes::core::TIMEOUT);
    }

    #[tokio::test]
    async fn closing_fails_pending_calls() {
        let (client, server) = pair();
        let call = tokio::spawn({
            let client = client.clone();
            async move { client.call_raw("slow", serde_json::json!({}), CallOptions::default()).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        server.close();
        let e = call.await.expect("join").expect_err("closed");
        assert_eq!(e.to_error().code(), codes::ipc::CONNECTION_CLOSED);
        tokio::time::timeout(Duration::from_secs(2), client.closed()).await.expect("client sees the close");
    }

    #[tokio::test]
    async fn server_requests_use_string_ids_and_reach_client_callbacks() {
        let (client, server) = pair();
        client.on_request::<russet_protocol::ui::UiConfirm, _, _>(|p| async move {
            Ok(russet_protocol::ui::UiConfirmResult { confirmed: p.danger })
        });
        let params = russet_protocol::ui::UiConfirmParams {
            title: "t".into(),
            message: "m".into(),
            detail: None,
            danger: true,
            requested_by: russet_protocol::common::ClientRef { kind: ClientKind::Cli, name: "x".into() },
            timeout_ms: 1000,
        };
        let r = server.call::<russet_protocol::ui::UiConfirm>(params).await.expect("answered");
        assert!(r.confirmed);
        let e =
            server.call_raw("ui.unknown", serde_json::json!({}), CallOptions::default()).await.expect_err("unknown");
        assert_eq!(e.code, crate::message::METHOD_NOT_FOUND);
    }
}
