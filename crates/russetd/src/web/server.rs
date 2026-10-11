// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The axum listener on `127.0.0.1:0` and its checks (ARCHITECTURE §6.6, §12.2 "Web endpoint").
//!
//! - **Host:** only `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>` (DNS rebinding).
//! - **Origin:** absent (non-browser clients), this endpoint's own origin, or a VS Code webview
//!   (`vscode-webview://…`, `vscode-file://…`); anything else is refused with 403.
//! - **Tokens:** [`TokenStore`] issues single-use 32-byte tokens with a deadline; routes redeem them (the
//!   mirror sends its token in the first message, never in the URL).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use russet_core::error::{Error, Result};
use sha2::Digest;
use tokio_util::sync::CancellationToken;

/// Binds `listen` and serves `routes` behind the Host and Origin checks until `shutdown`.
pub(crate) async fn start(listen: SocketAddr, routes: axum::Router, shutdown: CancellationToken) -> Result<SocketAddr> {
    if !listen.ip().is_loopback() {
        return Err(Error::invalid_argument(format!("the web endpoint only listens on loopback, not {listen}")));
    }
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| Error::io(format!("binding the web endpoint {listen}"), e))?;
    let addr = listener.local_addr().map_err(|e| Error::io("reading the web endpoint's address", e))?;
    let port = addr.port();
    let app = routes.layer(axum::middleware::from_fn(move |req: Request, next: Next| guard(port, req, next)));
    tokio::spawn(async move {
        let served = axum::serve(listener, app).with_graceful_shutdown(shutdown.cancelled_owned()).await;
        if let Err(e) = served {
            tracing::warn!("the web endpoint stopped: {e}");
        }
    });
    tracing::info!(%addr, "web endpoint started");
    Ok(addr)
}

async fn guard(port: u16, req: Request, next: Next) -> Response {
    match check_host_origin(port, req.headers()) {
        Ok(()) => next.run(req).await,
        Err(why) => (StatusCode::FORBIDDEN, why).into_response(),
    }
}

/// The Host and Origin rules of the module docs; `Err` names the refused value.
pub fn check_host_origin(port: u16, headers: &HeaderMap) -> std::result::Result<(), String> {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let allowed_hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")];
    if !allowed_hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
        return Err(format!("host `{host}` is not this endpoint"));
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let origin = origin.to_str().unwrap_or("");
        let own =
            [format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}"), format!("http://[::1]:{port}")];
        let ok = own.iter().any(|o| o.eq_ignore_ascii_case(origin))
            || origin.starts_with("vscode-webview://")
            || origin.starts_with("vscode-file://");
        if !ok {
            return Err(format!("origin `{origin}` may not use this endpoint"));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct Issued {
    scope: String,
    binding: String,
    deadline: Instant,
}

/// Single-use tokens: 32 random bytes as 64 hex digits, stored by their SHA-256 (module docs).
#[derive(Debug, Default)]
pub struct TokenStore {
    issued: Mutex<BTreeMap<String, Issued>>,
}

fn key(token: &str) -> String {
    sha2::Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

impl TokenStore {
    /// Issues a token for `scope` (a route) bound to `binding` (for example a stream id), valid for `ttl`.
    pub fn issue(&self, scope: &str, binding: impl Into<String>, ttl: Duration) -> String {
        let mut raw = [0u8; 32];
        russet_core::time::os_fill(&mut raw);
        let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let now = Instant::now();
        let mut issued = self.issued.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        issued.retain(|_, i| i.deadline > now);
        issued.insert(key(&token), Issued { scope: scope.to_owned(), binding: binding.into(), deadline: now + ttl });
        token
    }

    /// Redeems a token once: its binding when it is valid for `scope` and not expired, else `None`.
    pub fn redeem(&self, token: &str, scope: &str) -> Option<String> {
        let mut issued = self.issued.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let i = issued.remove(&key(token))?;
        (i.scope == scope && i.deadline > Instant::now()).then_some(i.binding)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(host: &str, origin: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_str(host).expect("header"));
        if let Some(o) = origin {
            h.insert(header::ORIGIN, HeaderValue::from_str(o).expect("header"));
        }
        h
    }

    #[test]
    fn hosts_and_origins_are_checked() {
        assert!(check_host_origin(4100, &headers("127.0.0.1:4100", None)).is_ok());
        assert!(check_host_origin(4100, &headers("localhost:4100", Some("http://localhost:4100"))).is_ok());
        assert!(check_host_origin(4100, &headers("127.0.0.1:4100", Some("vscode-webview://abc"))).is_ok());
        assert!(check_host_origin(4100, &headers("evil.example:4100", None)).is_err(), "DNS rebinding");
        assert!(check_host_origin(4100, &headers("127.0.0.1:4101", None)).is_err());
        assert!(check_host_origin(4100, &headers("127.0.0.1:4100", Some("https://evil.example"))).is_err());
    }

    #[test]
    fn tokens_are_single_use_scoped_and_expire() {
        let store = TokenStore::default();
        let t = store.issue("/mirror/v1", "str_1", Duration::from_secs(60));
        assert_eq!(t.len(), 64);
        assert_eq!(store.redeem(&t, "/mcp"), None, "wrong scope (and now spent)");
        let t = store.issue("/mirror/v1", "str_1", Duration::from_secs(60));
        assert_eq!(store.redeem(&t, "/mirror/v1").as_deref(), Some("str_1"));
        assert_eq!(store.redeem(&t, "/mirror/v1"), None, "single use");
        let t = store.issue("/mirror/v1", "str_2", Duration::ZERO);
        assert_eq!(store.redeem(&t, "/mirror/v1"), None, "expired");
    }
}
