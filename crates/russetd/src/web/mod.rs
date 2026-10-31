// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The web endpoint (ARCHITECTURE §6.6): one axum listener on `127.0.0.1:0`, started lazily, with a route
//! registry. Every route checks `Origin` and `Host` and needs a token.
//!
//! W2 owns `mod.rs` and `server.rs`; `mirror.rs` belongs to W10 and `mcp.rs` to W20. Areas register their routes
//! with [`WebEndpoint::route`] before the first [`WebEndpoint::ensure_started`]; the URL then goes into
//! `russetd.json` (`web`) and `initialize.result.endpoints.web`. Wave 1 registers no routes.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use russet_core::error::{Error, Result, codes};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

/// The opt-in streamable-HTTP MCP route (pre-created for W20).
pub mod mcp;
/// The `russet.mirror.v1` WebSocket route (pre-created for W10).
pub mod mirror;
/// The listener and route registry (W2).
pub mod server;

pub use server::TokenStore;

type StartHook = Box<dyn Fn(&str) + Send + Sync>;

struct Inner {
    listen: Mutex<SocketAddr>,
    routes: Mutex<Option<axum::Router>>,
    started: OnceCell<SocketAddr>,
    shutdown: CancellationToken,
    tokens: TokenStore,
    on_start: Mutex<Option<StartHook>>,
}

/// A cheap-to-clone handle on the web endpoint (`DaemonCtx.web`).
#[derive(Clone)]
pub struct WebEndpoint {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for WebEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebEndpoint").field("listen", &self.listen()).field("url", &self.url()).finish()
    }
}

impl Default for WebEndpoint {
    fn default() -> Self {
        Self::new(SocketAddr::from(([127, 0, 0, 1], 0)), CancellationToken::new())
    }
}

impl WebEndpoint {
    /// An endpoint that will listen on `listen` (`[web] listen`, default `127.0.0.1:0`) until `shutdown`.
    pub fn new(listen: SocketAddr, shutdown: CancellationToken) -> Self {
        Self {
            inner: Arc::new(Inner {
                listen: Mutex::new(listen),
                routes: Mutex::new(Some(axum::Router::new())),
                started: OnceCell::new(),
                shutdown,
                tokens: TokenStore::default(),
                on_start: Mutex::new(None),
            }),
        }
    }

    /// The address the endpoint listens on, or will.
    pub fn listen(&self) -> SocketAddr {
        *self.inner.listen.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Changes the address (`[web] listen`) if the endpoint has not started; returns whether it applied.
    pub fn set_listen(&self, addr: SocketAddr) -> bool {
        if self.inner.started.initialized() {
            return false;
        }
        *self.inner.listen.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = addr;
        true
    }

    /// Registers `router` under `path` (for example `/mirror/v1`); only before the endpoint starts.
    pub fn route(&self, path: &str, router: axum::routing::MethodRouter) -> Result<()> {
        let mut routes = self.inner.routes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(current) = routes.take() else {
            return Err(Error::new(codes::core::BUSY, format!("the web endpoint already runs; {path} came too late"))
                .with_hint("Register web routes while the daemon starts."));
        };
        *routes = Some(current.route(path, router));
        Ok(())
    }

    /// Called with the URL once the endpoint starts (the daemon rewrites `russetd.json`).
    pub fn on_start(&self, hook: impl Fn(&str) + Send + Sync + 'static) {
        *self.inner.on_start.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(hook));
    }

    /// Starts the listener on first use and returns its address.
    pub async fn ensure_started(&self) -> Result<SocketAddr> {
        let inner = Arc::clone(&self.inner);
        let addr = self
            .inner
            .started
            .get_or_try_init(|| async move {
                let routes =
                    inner.routes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take().unwrap_or_default();
                let listen = *inner.listen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                server::start(listen, routes, inner.shutdown.clone()).await
            })
            .await
            .copied()?;
        if let Some(hook) = self.inner.on_start.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            hook(&format!("http://{addr}"));
        }
        Ok(addr)
    }

    /// `http://127.0.0.1:<port>` once started.
    pub fn url(&self) -> Option<String> {
        self.inner.started.get().map(|a| format!("http://{a}"))
    }

    /// Single-use tokens for routes (the mirror's first message, ARCHITECTURE §10.1).
    pub fn tokens(&self) -> &TokenStore {
        &self.inner.tokens
    }
}
