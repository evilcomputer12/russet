// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`Router`]: method name → typed handler (ARCHITECTURE §5.3 `russet-ipc`, [D 93]).
//!
//! Handlers are registered against the `rpc_method!` markers, so the router knows every method's
//! [`MethodInfo`] (ceiling, per-call class, long-running flag) without generic code per method. Params that
//! do not deserialize answer JSON-RPC `-32602` with `data.details.path` (ARCHITECTURE §13.1); handlers keep
//! `core.invalidArgument` (1006) for semantic validation. An optional [`Layer`] wraps every dispatch: the
//! daemon uses it to run long-running methods as jobs and to audit calls.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use russet_core::error::Error;
use russet_protocol::{MethodInfo, RpcMethod, RpcNotification};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::message::{RequestId, RpcError};
use crate::peer::{BoxFuture, Peer, ProgressReporter};

/// What a request handler gets besides its context and params.
#[derive(Debug, Clone)]
pub struct RequestCtx {
    /// The requesting connection (send it notifications or `ui.*` requests).
    pub peer: Peer,
    /// Fired by `$/cancelRequest`, by the connection closing, or (in a job) by `job.cancel`.
    pub cancel: CancellationToken,
    /// `$/progress` to the requester, plus the job's event log inside a job; an [`EventSink`] for domain APIs.
    ///
    /// [`EventSink`]: russet_core::progress::EventSink
    pub progress: ProgressReporter,
    pub request_id: RequestId,
}

/// What a notification handler gets besides its context and params.
#[derive(Debug, Clone)]
pub struct ConnCtx {
    pub peer: Peer,
}

type RequestHandler<C> = Arc<dyn Fn(C, RequestCtx, Value) -> BoxFuture<Result<Value, RpcError>> + Send + Sync>;
type NotificationHandler<C> = Arc<dyn Fn(C, ConnCtx, Value) -> BoxFuture<()> + Send + Sync>;

/// Runs the handler a [`Layer`] wraps.
pub struct Next<C> {
    handler: RequestHandler<C>,
}

impl<C> Next<C> {
    /// Runs the wrapped handler.
    pub fn run(self, cx: C, rq: RequestCtx, params: Value) -> BoxFuture<Result<Value, RpcError>> {
        (self.handler)(cx, rq, params)
    }
}

impl<C> fmt::Debug for Next<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Next")
    }
}

/// Wraps every dispatched request (after authorization).
pub trait Layer<C>: Send + Sync + 'static {
    fn call(
        &self,
        cx: C,
        rq: RequestCtx,
        info: MethodInfo,
        params: Value,
        next: Next<C>,
    ) -> BoxFuture<Result<Value, RpcError>>;
}

/// Method name → typed handler.
pub struct Router<C: Clone + Send + Sync + 'static> {
    requests: BTreeMap<&'static str, (MethodInfo, RequestHandler<C>)>,
    notifications: BTreeMap<&'static str, NotificationHandler<C>>,
    layer: Option<Arc<dyn Layer<C>>>,
}

/// Deserializes `params` for `method`, naming the offending field on failure (`-32602`).
pub(crate) fn parse_params<T: DeserializeOwned>(method: &str, params: Value) -> Result<T, RpcError> {
    serde_path_to_error::deserialize(params).map_err(|e| {
        let path = e.path().to_string();
        RpcError::invalid_params(method, &path, e.into_inner())
    })
}

impl<C: Clone + Send + Sync + 'static> Router<C> {
    pub fn new() -> Self {
        Self { requests: BTreeMap::new(), notifications: BTreeMap::new(), layer: None }
    }

    /// Registers the handler of method `M`. Registering a name twice is a programming error.
    pub fn request<M, F, Fut>(&mut self, handler: F) -> &mut Self
    where
        M: RpcMethod,
        F: Fn(C, RequestCtx, M::Params) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = russet_core::Result<M::Result>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let erased: RequestHandler<C> = Arc::new(move |cx, rq, params| {
            let handler = Arc::clone(&handler);
            Box::pin(async move {
                let params: M::Params = parse_params(M::NAME, params)?;
                let result = handler(cx, rq, params).await.map_err(RpcError::from)?;
                serde_json::to_value(result)
                    .map_err(|e| RpcError::internal(format!("cannot serialize the result of {}: {e}", M::NAME)))
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

    /// Wraps every request dispatch in `layer` (one layer; a later call replaces it).
    pub fn layer(&mut self, layer: Arc<dyn Layer<C>>) -> &mut Self {
        self.layer = Some(layer);
        self
    }

    /// Every registered method (name, permission, long-running, schema hooks).
    pub fn methods(&self) -> impl Iterator<Item = MethodInfo> + '_ {
        self.requests.values().map(|(info, _)| *info)
    }

    /// The description of `method`, if it is registered.
    pub fn method_info(&self, method: &str) -> Option<MethodInfo> {
        self.requests.get(method).map(|(info, _)| *info)
    }

    /// Runs the handler of `method` (through the layer); `None` when no handler is registered (JSON-RPC
    /// `-32601`).
    pub fn dispatch(
        &self,
        cx: C,
        rq: RequestCtx,
        method: &str,
        params: Value,
    ) -> Option<BoxFuture<Result<Value, RpcError>>> {
        let (info, handler) = self.requests.get(method)?;
        let handler = Arc::clone(handler);
        Some(match &self.layer {
            Some(layer) => layer.call(cx, rq, *info, params, Next { handler }),
            None => handler(cx, rq, params),
        })
    }

    /// Runs the handler of notification `method`, if any.
    pub fn dispatch_notification(&self, cx: C, conn: ConnCtx, method: &str, params: Value) -> Option<BoxFuture<()>> {
        let handler = self.notifications.get(method)?;
        Some(handler(cx, conn, params))
    }
}

impl<C: Clone + Send + Sync + 'static> Default for Router<C> {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds an `Error` result for a handler from a [`RpcError`] (a client-side helper).
pub fn rpc_to_error(e: &RpcError) -> Error {
    e.to_error()
}
