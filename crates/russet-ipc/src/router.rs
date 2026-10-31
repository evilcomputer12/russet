// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`Router`]: method name → typed handler (ARCHITECTURE §5.3 `russet-ipc`, [D 93]).
//!
//! Handlers are registered against the `rpc_method!` markers, so the router knows every method's
//! [`MethodInfo`] (ceiling, per-call class, long-running flag) without generic code per method. Params that
//! do not deserialize answer JSON-RPC `-32602` with `data.details.path` (ARCHITECTURE §13.1); handlers keep
//! `core.invalidArgument` (1006) for semantic validation. An optional [`Layer`] wraps every dispatch: the
//! daemon uses it to run long-running methods as jobs and to audit calls.
//!
//! [`Extensions`] carry whatever else a registration declares about its methods. The handler seam hands an
//! area nothing but `register(r: &mut Router<DaemonCtx>)` (ARCHITECTURE §5.3), so russetd's registration hooks
//! (job declarations, typed-confirmation texts, ownership resolvers, service factories) store their values on
//! the router, and the daemon takes them off the router it built.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
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

/// Typed values, one per type, that registrations leave on a [`Router`] for the server that owns it (module
/// docs).
#[derive(Default)]
pub struct Extensions {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl fmt::Debug for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extensions").field("len", &self.map.len()).finish()
    }
}

impl Extensions {
    /// Stores `value`, returning the value of the same type it replaces.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.map.insert(TypeId::of::<T>(), Box::new(value)).and_then(|old| old.downcast::<T>().ok().map(|b| *b))
    }

    /// The value of type `T`.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.map.get(&TypeId::of::<T>()).and_then(|b| b.downcast_ref::<T>())
    }

    /// The value of type `T`, mutably.
    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.map.get_mut(&TypeId::of::<T>()).and_then(|b| b.downcast_mut::<T>())
    }

    /// The value of type `T`, inserting `T::default()` first when there is none.
    pub fn get_or_default<T: Default + Send + Sync + 'static>(&mut self) -> &mut T {
        let slot = self.map.entry(TypeId::of::<T>()).or_insert_with(|| Box::new(T::default()));
        match slot.downcast_mut::<T>() {
            Some(v) => v,
            None => unreachable!("extensions are keyed by their own type"),
        }
    }

    /// Takes the value of type `T` out.
    pub fn remove<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.map.remove(&TypeId::of::<T>()).and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    /// Whether nothing is stored.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Method name → typed handler.
pub struct Router<C: Clone + Send + Sync + 'static> {
    requests: BTreeMap<&'static str, (MethodInfo, RequestHandler<C>)>,
    notifications: BTreeMap<&'static str, NotificationHandler<C>>,
    layer: Option<Arc<dyn Layer<C>>>,
    extensions: Extensions,
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
        Self {
            requests: BTreeMap::new(),
            notifications: BTreeMap::new(),
            layer: None,
            extensions: Extensions::default(),
        }
    }

    /// What registrations declared besides their handlers (module docs).
    pub fn extensions(&self) -> &Extensions {
        &self.extensions
    }

    /// [`Router::extensions`], mutably: registration hooks store their values here.
    pub fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.extensions
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq)]
    struct Declared(Vec<&'static str>);

    #[test]
    fn extensions_hold_one_value_per_type() {
        let mut r = Router::<()>::new();
        assert!(r.extensions().is_empty());
        r.extensions_mut().get_or_default::<Declared>().0.push("build.start");
        r.extensions_mut().get_or_default::<Declared>().0.push("sdk.import");
        assert_eq!(r.extensions().get::<Declared>(), Some(&Declared(vec!["build.start", "sdk.import"])));
        assert_eq!(r.extensions_mut().insert(7u32), None);
        assert_eq!(r.extensions_mut().insert(8u32), Some(7));
        assert_eq!(r.extensions_mut().remove::<Declared>().map(|d| d.0.len()), Some(2));
        assert!(r.extensions().get::<Declared>().is_none());
        assert_eq!(r.extensions().get::<u32>(), Some(&8));
    }
}
