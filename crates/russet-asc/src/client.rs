// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The transport (SIGN §5.3): pluggable [`Auth`], the per-key [`RateLimiter`] (a token bucket that also follows
//! `X-Rate-Limit`), retries for GETs (5xx and transport errors; 429 with exponential backoff), App Store Connect
//! error mapping and JSON:API pagination that keeps `limit` next to `cursor`. Wave 1 is GET-only: the client has
//! no method that sends anything else. With `RUSSET_OFFLINE=1` only loopback hosts are reached (T3).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russet_core::Result;
use russet_core::error::{Error, codes};
use secrecy::ExposeSecret as _;
use serde::de::DeserializeOwned;
use url::Url;

use crate::limiter::RateLimiter;
use crate::model::{Document, Resource};
use crate::token::{ScopeEntry, TokenFactory};

/// Which API a client talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiFlavor {
    /// The public App Store Connect API (team or individual key).
    Public,
    /// developerservices2 with a free Apple ID session (W22).
    XcodeDs2 { team_id: String },
}

/// How requests are authenticated (team-key JWT, or the free Apple ID's DS2 session, W22).
#[async_trait]
pub trait Auth: Send + Sync {
    /// The API root, for example `https://api.appstoreconnect.apple.com/`.
    fn base_url(&self) -> &Url;
    fn flavor(&self) -> ApiFlavor;
    /// Adds credentials (headers) to a request about to be sent.
    async fn authorize(&self, req: &mut reqwest::Request) -> Result<()>;
}

/// `<path>?<query>` of a request URL, as a scope entry names it.
pub fn path_and_query(url: &Url) -> String {
    match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    }
}

/// Which token a [`JwtAuth`] sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenMode {
    /// One cached, unscoped 19-minute token.
    Bearer,
    /// A token scoped to exactly the request being sent (`GET <path>?<query>`): App Store Connect itself then
    /// refuses anything but that read [V3]. The Wave-1 default.
    PerRequest { lifetime: Duration },
    /// One scoped token for every request (dashboards, tests).
    Fixed { scope: Vec<ScopeEntry>, lifetime: Duration },
}

/// JWT authentication with an App Store Connect key.
pub struct JwtAuth {
    base: Url,
    tokens: Arc<TokenFactory>,
    mode: TokenMode,
}

impl JwtAuth {
    pub fn new(base: Url, tokens: Arc<TokenFactory>, mode: TokenMode) -> Self {
        Self { base, tokens, mode }
    }

    /// Per-request GET scopes with a 10-minute lifetime (Wave 1).
    pub fn per_request(base: Url, tokens: Arc<TokenFactory>) -> Self {
        Self::new(base, tokens, TokenMode::PerRequest { lifetime: Duration::from_secs(600) })
    }

    /// One cached, unscoped bearer token (SIGN §5.2). App Store Connect refuses non-GET requests under a scoped
    /// token, so an approved mutation (W12) uses this mode, which also sends GETs. The caller gates the writes
    /// behind the permission engine and an approval.
    pub fn bearer(base: Url, tokens: Arc<TokenFactory>) -> Self {
        Self::new(base, tokens, TokenMode::Bearer)
    }

    pub fn tokens(&self) -> &Arc<TokenFactory> {
        &self.tokens
    }
}

#[async_trait]
impl Auth for JwtAuth {
    fn base_url(&self) -> &Url {
        &self.base
    }

    fn flavor(&self) -> ApiFlavor {
        ApiFlavor::Public
    }

    async fn authorize(&self, req: &mut reqwest::Request) -> Result<()> {
        if req.method() != reqwest::Method::GET && !matches!(self.mode, TokenMode::Bearer) {
            return Err(Error::new(
                codes::core::INVALID_ARGUMENT,
                "a scoped App Store Connect token sends GET requests only",
            )
            .with_hint("Mutations go through an approved provisioning plan over a bearer-token client (W12)."));
        }
        let token = match &self.mode {
            TokenMode::Bearer => self.tokens.bearer()?,
            TokenMode::PerRequest { lifetime } => {
                self.tokens.scoped(&[ScopeEntry::get(&path_and_query(req.url()))], *lifetime)?
            }
            TokenMode::Fixed { scope, lifetime } => self.tokens.scoped(scope, *lifetime)?,
        };
        let value = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
            .map_err(|_| Error::new(codes::core::INTERNAL, "a minted token is not a valid header value"))?;
        req.headers_mut().insert(reqwest::header::AUTHORIZATION, value);
        Ok(())
    }
}

/// Retry and pacing parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Attempts for a GET that fails with 5xx or a transport error.
    pub attempts: u32,
    /// Attempts for a GET answered 429.
    pub rate_limited_attempts: u32,
    /// The first backoff; it doubles up to [`Self::max_backoff`].
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    /// Below this many remaining requests of the hourly quota, every request waits [`Self::low_quota_delay`].
    pub low_quota_threshold: u32,
    pub low_quota_delay: Duration,
    pub timeout: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            rate_limited_attempts: 6,
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            low_quota_threshold: 200,
            low_quota_delay: Duration::from_secs(1),
            timeout: Duration::from_secs(60),
        }
    }
}

/// The client's view of the hourly quota (3,600 requests per hour per key [V2]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Quota {
    pub limit: Option<u32>,
    pub remaining: Option<u32>,
}

/// Parses `X-Rate-Limit: user-hour-lim:3600;user-hour-rem:3599;`.
pub fn parse_rate_limit(header: &str) -> Quota {
    let mut q = Quota::default();
    for part in header.split(';') {
        let mut kv = part.trim().splitn(2, ':');
        match (kv.next(), kv.next().and_then(|v| v.trim().parse::<u32>().ok())) {
            (Some("user-hour-lim"), Some(v)) => q.limit = Some(v),
            (Some("user-hour-rem"), Some(v)) => q.remaining = Some(v),
            _ => {}
        }
    }
    q
}

/// The typed App Store Connect client. Every request is a GET in Wave 1.
pub struct AscClient {
    http: reqwest::Client,
    auth: Arc<dyn Auth>,
    policy: RetryPolicy,
    limiter: Arc<RateLimiter>,
}

impl std::fmt::Debug for AscClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AscClient").field("base", &self.auth.base_url().as_str()).finish_non_exhaustive()
    }
}

/// The API paths `raw_get` (and `asc.get`, W12) may read [SIGN §11].
const RAW_GET_ALLOWLIST: &[&str] = &[
    "/v1/apps",
    "/v1/builds",
    "/v1/betaGroups",
    "/v1/bundleIds",
    "/v1/profiles",
    "/v1/devices",
    "/v1/certificates",
    "/v1/ci",
];

impl AscClient {
    pub fn new(auth: Arc<dyn Auth>) -> Result<Self> {
        Self::with_policy(auth, RetryPolicy::default())
    }

    pub fn with_policy(auth: Arc<dyn Auth>, policy: RetryPolicy) -> Result<Self> {
        Self::with_limiter(auth, policy, Arc::new(RateLimiter::per_key()))
    }

    /// A client that shares `limiter` with every other client of the same key, so the hourly quota spans them.
    pub fn with_limiter(auth: Arc<dyn Auth>, policy: RetryPolicy, limiter: Arc<RateLimiter>) -> Result<Self> {
        // One rustls provider for the process: ring (ARCHITECTURE §16.6). Binaries install it at start; this
        // covers library callers and tests, and is a no-op when it is already installed.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("russet/", env!("CARGO_PKG_VERSION")))
            .timeout(policy.timeout)
            .build()
            .map_err(|e| {
                Error::new(codes::core::INTERNAL, format!("cannot build the HTTP client: {e}")).caused_by(e)
            })?;
        Ok(Self { http, auth, policy, limiter })
    }

    pub fn flavor(&self) -> ApiFlavor {
        self.auth.flavor()
    }

    /// The last quota App Store Connect reported (shared with the other clients of the key).
    pub fn quota(&self) -> Quota {
        self.limiter.quota()
    }

    /// The key's rate limiter.
    pub fn limiter(&self) -> &Arc<RateLimiter> {
        &self.limiter
    }

    /// GETs `path_and_query` (`/v1/…`) and decodes the JSON body.
    pub async fn get_json<T: DeserializeOwned>(&self, path_and_query: &str) -> Result<T> {
        let (_, body) = self.send_get(path_and_query, true).await?;
        serde_json::from_slice(&body).map_err(|e| {
            Error::new(
                codes::account::APPLE_SERVICE_UNAVAILABLE,
                format!("unexpected App Store Connect response for {}: {e}", strip_query(path_and_query)),
            )
            .with_hint("App Store Connect changed its response format, or a proxy answered; retry later.")
        })
    }

    /// GETs one resource document.
    pub async fn get_resource<A: DeserializeOwned>(&self, path_and_query: &str) -> Result<Document<Resource<A>>> {
        self.get_json(path_and_query).await
    }

    /// GETs every page of a collection, following `links.next` (which must stay on the same origin) and
    /// keeping `limit` next to `cursor`. Returns the resources and every `included` object.
    pub async fn get_all<A: DeserializeOwned>(
        &self,
        path_and_query: &str,
    ) -> Result<(Vec<Resource<A>>, Vec<serde_json::Value>)> {
        let mut out = Vec::new();
        let mut included = Vec::new();
        let limit = query_param(path_and_query, "limit");
        let mut next = Some(path_and_query.to_owned());
        let mut pages = 0;
        while let Some(pq) = next.take() {
            pages += 1;
            if pages > 1000 {
                return Err(Error::new(
                    codes::account::APPLE_SERVICE_UNAVAILABLE,
                    "App Store Connect paging did not end",
                )
                .with_hint("Retry later; report it with `russet bugreport` if it persists."));
            }
            let doc: Document<Vec<Resource<A>>> = self.get_json(&pq).await?;
            out.extend(doc.data);
            included.extend(doc.included);
            if let Some(link) = doc.links.and_then(|l| l.next) {
                let mut rel = self.same_origin_path(&link)?;
                if let Some(l) = &limit
                    && query_param(&rel, "limit").is_none()
                {
                    rel.push(if rel.contains('?') { '&' } else { '?' });
                    rel.push_str("limit=");
                    rel.push_str(l);
                }
                next = Some(rel);
            }
        }
        Ok((out, included))
    }

    /// POSTs a JSON:API request body to `path` (`/v1/…`) and decodes the created resource document. The client
    /// must be in bearer mode (an approved mutation, W12); a scoped client refuses non-GET.
    pub async fn post_json<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> Result<T> {
        let bytes = serde_json::to_vec(body)
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode a request body: {e}")))?;
        let (_, body) = self.send_mutation(reqwest::Method::POST, path, Some(bytes)).await?;
        serde_json::from_slice(&body).map_err(|e| {
            Error::new(
                codes::account::APPLE_SERVICE_UNAVAILABLE,
                format!("unexpected App Store Connect response for {}: {e}", strip_query(path)),
            )
            .with_hint("App Store Connect changed its response format, or a proxy answered; retry later.")
        })
    }

    /// Sends an HTTP DELETE to `path` (`/v1/…/{id}`); App Store Connect answers 204 No Content.
    pub async fn delete(&self, path: &str) -> Result<()> {
        self.send_mutation(reqwest::Method::DELETE, path, None).await?;
        Ok(())
    }

    /// An allowlisted GET returning the status and JSON body (`asc.get`, W12). Errors are returned as the
    /// status and body, not mapped. The allowlist is checked on the URL as it will be sent ([`Self::raw_get_url`]).
    pub async fn raw_get(&self, path_and_query: &str) -> Result<(u16, serde_json::Value)> {
        let url = self.raw_get_url(path_and_query)?;
        let (status, body) = self.send_url(url, path_and_query, false).await?;
        let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        Ok((status, json))
    }

    /// The URL [`Self::raw_get`] sends for `path_and_query`: a `/v1/…` path on the API's own origin that names an
    /// allowlisted resource after the URL parser has normalized it, and that normalization left unchanged. Encoded
    /// dots, slashes and backslashes (`%2e`, `%2f`, `%5c`), backslashes, `..`, empty segments and fragments are
    /// refused before parsing, so no spelling of a path can reach a resource outside the allowlist.
    pub fn raw_get_url(&self, path_and_query: &str) -> Result<Url> {
        let path_part = strip_query(path_and_query);
        let denied = |why: &str| {
            Error::new(codes::perm::DENIED, format!("{path_part} is not on the read allowlist ({why})")).with_hint(
                "Allowed: /v1/apps, /v1/builds, /v1/betaGroups, /v1/bundleIds, /v1/profiles, /v1/devices, /v1/certificates, /v1/ci*, spelled plainly.",
            )
        };
        let lower = path_and_query.to_ascii_lowercase();
        if !path_and_query.starts_with("/v1/") {
            return Err(denied("not a /v1/ path"));
        }
        if path_and_query.contains('\\')
            || path_and_query.contains('#')
            || ["%2e", "%2f", "%5c"].iter().any(|enc| lower.contains(enc))
            || path_part.contains("..")
            || path_part.contains("//")
        {
            return Err(denied("encoded or relative path segments"));
        }
        let base = self.auth.base_url();
        let url = base.join(path_and_query.trim_start_matches('/')).map_err(|_| denied("not a valid path"))?;
        if url.origin() != base.origin() {
            return Err(denied("another origin"));
        }
        let path = url.path().strip_prefix(base.path().trim_end_matches('/')).unwrap_or_default();
        if path != path_part {
            return Err(denied("the path normalizes to another resource"));
        }
        let allowed = RAW_GET_ALLOWLIST.iter().any(|p| {
            path == *p || path.starts_with(&format!("{p}/")) || (*p == "/v1/ci" && path.starts_with("/v1/ci"))
        });
        if !allowed {
            return Err(denied("not an allowlisted resource"));
        }
        Ok(url)
    }

    /// Converts an absolute `links.next` URL to a path and query on our base origin.
    fn same_origin_path(&self, link: &str) -> Result<String> {
        let base = self.auth.base_url();
        let url =
            base.join(link).map_err(|_| Error::new(codes::account::APPLE_SERVICE_UNAVAILABLE, "invalid next link"))?;
        if url.origin() != base.origin() {
            return Err(Error::new(
                codes::account::APPLE_SERVICE_UNAVAILABLE,
                "App Store Connect pointed a next link at another host; refusing to send the token there",
            )
            .with_hint("Check RUSSET_ENDPOINTS_FILE and any proxy in between; report it with `russet bugreport`."));
        }
        let mut pq = url.path().to_owned();
        if let Some(q) = url.query() {
            pq.push('?');
            pq.push_str(q);
        }
        Ok(pq)
    }

    async fn pace(&self) {
        let low = self.limiter.quota().remaining.is_some_and(|r| r < self.policy.low_quota_threshold);
        if low {
            tokio::time::sleep(self.policy.low_quota_delay).await;
        }
        self.limiter.acquire().await;
    }

    /// Sends a GET with retries. `map_errors`: turn non-2xx answers into Russet errors.
    async fn send_get(&self, path_and_query: &str, map_errors: bool) -> Result<(u16, Vec<u8>)> {
        let url = self.auth.base_url().join(path_and_query.trim_start_matches('/')).map_err(|e| {
            Error::new(codes::core::INVALID_ARGUMENT, format!("invalid App Store Connect path {path_and_query}: {e}"))
        })?;
        self.send_url(url, path_and_query, map_errors).await
    }

    /// Sends a GET of `url` with retries; `label` names it in errors (never the query, which can name devices).
    async fn send_url(&self, url: Url, label: &str, map_errors: bool) -> Result<(u16, Vec<u8>)> {
        refuse_offline(&url)?;
        let path_and_query = label;
        let mut attempt = 0u32;
        let mut rate_attempt = 0u32;
        loop {
            self.pace().await;
            let mut req = self.http.get(url.clone()).build().map_err(|e| {
                Error::new(codes::core::INTERNAL, format!("cannot build a request: {}", without_url(&e)))
            })?;
            self.auth.authorize(&mut req).await?;
            let sent = self.http.execute(req).await;
            let resp = match sent {
                Ok(r) => r,
                Err(e) => {
                    attempt += 1;
                    if attempt < self.policy.attempts {
                        tokio::time::sleep(self.backoff(attempt)).await;
                        continue;
                    }
                    return Err(Error::new(
                        codes::account::APPLE_SERVICE_UNAVAILABLE,
                        format!("cannot reach App Store Connect: {}", without_url(&e)),
                    )
                    .with_hint("Check the network; `russet doctor --online` tests the connection.")
                    .retryable(true));
                }
            };
            let status = resp.status().as_u16();
            if let Some(q) = resp.headers().get("x-rate-limit").and_then(|v| v.to_str().ok()).map(parse_rate_limit) {
                self.limiter.observe(q);
            }
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            if status == 429 {
                rate_attempt += 1;
                if rate_attempt < self.policy.rate_limited_attempts {
                    let wait = retry_after.unwrap_or_else(|| self.backoff(rate_attempt)).min(self.policy.max_backoff);
                    tokio::time::sleep(wait).await;
                    continue;
                }
            } else if (500..600).contains(&status) {
                attempt += 1;
                if attempt < self.policy.attempts {
                    tokio::time::sleep(self.backoff(attempt)).await;
                    continue;
                }
            }
            if !map_errors || (200..300).contains(&status) {
                return Ok((status, body));
            }
            return Err(map_status(status, &body, path_and_query));
        }
    }

    /// Sends a mutating request (POST/DELETE) with a JSON:API `application/json` body. It is not retried on a
    /// transport error or 5xx (mutations are not idempotent and may have been applied); a 429 is retried, since
    /// App Store Connect rejected it before processing. Non-2xx answers are mapped to Russet errors.
    async fn send_mutation(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<(u16, Vec<u8>)> {
        let url = self.auth.base_url().join(path.trim_start_matches('/')).map_err(|e| {
            Error::new(codes::core::INVALID_ARGUMENT, format!("invalid App Store Connect path {path}: {e}"))
        })?;
        refuse_offline(&url)?;
        let mut rate_attempt = 0u32;
        loop {
            self.pace().await;
            let mut builder = self.http.request(method.clone(), url.clone());
            if let Some(b) = &body {
                builder = builder.header(reqwest::header::CONTENT_TYPE, "application/json").body(b.clone());
            }
            let mut req = builder.build().map_err(|e| {
                Error::new(codes::core::INTERNAL, format!("cannot build a request: {}", without_url(&e)))
            })?;
            self.auth.authorize(&mut req).await?;
            let resp = self.http.execute(req).await.map_err(|e| {
                Error::new(
                    codes::account::APPLE_SERVICE_UNAVAILABLE,
                    format!("cannot reach App Store Connect: {}", without_url(&e)),
                )
                .with_hint("Check the network; `russet doctor --online` tests the connection.")
                .retryable(true)
            })?;
            let status = resp.status().as_u16();
            if let Some(q) = resp.headers().get("x-rate-limit").and_then(|v| v.to_str().ok()).map(parse_rate_limit) {
                self.limiter.observe(q);
            }
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            if status == 429 {
                rate_attempt += 1;
                if rate_attempt < self.policy.rate_limited_attempts {
                    let wait = retry_after.unwrap_or_else(|| self.backoff(rate_attempt)).min(self.policy.max_backoff);
                    tokio::time::sleep(wait).await;
                    continue;
                }
            }
            if (200..300).contains(&status) {
                return Ok((status, body));
            }
            return Err(map_status(status, &body, strip_query(path)));
        }
    }

    /// Exponential backoff with up to 25 % jitter, capped.
    fn backoff(&self, attempt: u32) -> Duration {
        let base = self.policy.base_backoff.saturating_mul(1u32 << attempt.saturating_sub(1).min(16));
        let capped = base.min(self.policy.max_backoff);
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let jitter = capped.mul_f64(f64::from(nanos % 250) / 1000.0);
        capped + jitter
    }
}

/// Whether `RUSSET_OFFLINE=1` is set (T3: offline runs reach loopback hosts only).
pub fn offline() -> bool {
    std::env::var_os("RUSSET_OFFLINE").is_some_and(|v| v == "1")
}

/// Whether `u` names this machine (`localhost`, 127.0.0.0/8 or `::1`).
pub fn is_loopback(u: &Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn refuse_offline(url: &Url) -> Result<()> {
    if !offline() || is_loopback(url) {
        return Ok(());
    }
    Err(Error::new(
        codes::core::INVALID_ARGUMENT,
        format!("RUSSET_OFFLINE=1 refuses the non-loopback host {}", url.host_str().unwrap_or("?")),
    )
    .with_hint("Point App Store Connect at a local fake with RUSSET_ENDPOINTS_FILE, or unset RUSSET_OFFLINE."))
}

fn strip_query(pq: &str) -> &str {
    pq.split('?').next().unwrap_or(pq)
}

fn query_param(pq: &str, name: &str) -> Option<String> {
    let q = pq.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == name).then(|| v.to_owned())
    })
}

/// A transport error without its URL (URLs can carry filters naming bundle IDs; never the token).
fn without_url(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    if let Some(u) = e.url() {
        s = s.replace(u.as_str(), "<url>");
    }
    s
}

/// Maps an App Store Connect error response (SIGN §5.3) to a Russet error.
pub fn map_status(status: u16, body: &[u8], path_and_query: &str) -> Error {
    #[derive(serde::Deserialize)]
    struct Errs {
        #[serde(default)]
        errors: Vec<AppleError>,
    }
    #[derive(serde::Deserialize)]
    struct AppleError {
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        detail: Option<String>,
    }
    let first = serde_json::from_slice::<Errs>(body).ok().and_then(|e| e.errors.into_iter().next());
    let apple_code = first.as_ref().and_then(|e| e.code.clone()).unwrap_or_default();
    let detail = first.and_then(|e| e.detail).unwrap_or_default();
    let path = strip_query(path_and_query);
    let details = serde_json::json!({"status": status, "appleCode": apple_code, "path": path});
    let msg = |what: &str| {
        if detail.is_empty() {
            format!("{what} ({status} on {path})")
        } else {
            format!("{what} ({status} on {path}): {detail}")
        }
    };
    match status {
        401 => Error::new(codes::account::ASC_UNAUTHORIZED, msg("App Store Connect rejected the token"))
            .with_hint("Check the key ID and issuer ID, sync the clock (NTP), and re-add the key if it was revoked; tokens may live at most 20 minutes.")
            .with_details(details),
        403 if apple_code.contains("REQUEST_DOES_NOT_MATCH_SCOPE") => {
            Error::new(codes::account::ASC_SCOPE, msg("the scoped token does not cover this request"))
                .with_hint("This is a Russet bug: report it with `russet bugreport`.")
                .with_details(details)
        }
        // Apple's terms changed: no key or role helps until the account holder accepts them on the web
        // (developer services send PLA_NOT_ACCEPTED, the App Store Connect API REQUIRED_AGREEMENTS_…).
        403 if apple_code.contains("PLA_NOT_ACCEPTED") || apple_code.contains("REQUIRED_AGREEMENTS") => {
            Error::new(codes::account::ASC_FORBIDDEN, msg("Apple requires an updated developer agreement"))
                .with_hint("The account holder must accept the latest Apple Developer Program License Agreement: sign in at https://developer.apple.com/account, accept it, then retry.")
                .with_details(details)
        }
        403 => Error::new(codes::account::ASC_FORBIDDEN, msg("App Store Connect refused the request"))
            .with_hint("The key's role lacks this permission: use an Admin or App Manager key with access to Certificates, Identifiers & Profiles.")
            .with_details(details),
        404 => Error::new(codes::core::NOT_FOUND, msg("App Store Connect has no such resource")).with_details(details),
        429 => Error::new(codes::account::ASC_RATE_LIMITED, msg("App Store Connect rate-limited the key"))
            .with_hint("App Store Connect allows 3,600 requests per hour per key; wait and retry.")
            .with_details(details)
            .retryable(true),
        500..=599 => Error::new(codes::account::APPLE_SERVICE_UNAVAILABLE, msg("App Store Connect is unavailable"))
            .with_hint("Retry later; https://developer.apple.com/system-status/ shows outages.")
            .with_details(details)
            .retryable(true),
        _ => Error::new(codes::account::APPLE_SERVICE_UNAVAILABLE, msg("unexpected App Store Connect answer"))
            .with_hint("Retry later; report it if it persists.")
            .with_details(details),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_header() {
        let q = parse_rate_limit("user-hour-lim:3600;user-hour-rem:3599;");
        assert_eq!(q, Quota { limit: Some(3600), remaining: Some(3599) });
        assert_eq!(parse_rate_limit("garbage"), Quota::default());
    }

    #[test]
    fn query_helpers() {
        assert_eq!(query_param("/v1/x?limit=200&cursor=abc", "limit").as_deref(), Some("200"));
        assert_eq!(query_param("/v1/x", "limit"), None);
        assert_eq!(strip_query("/v1/x?y=1"), "/v1/x");
    }

    #[test]
    fn scope_entries_drop_paging_params() {
        assert_eq!(ScopeEntry::get("/v1/devices?limit=200&cursor=abc").0, "GET /v1/devices");
        assert_eq!(
            ScopeEntry::get("/v1/bundleIds?filter[identifier]=com.x&limit=200&include=bundleIdCapabilities").0,
            "GET /v1/bundleIds?filter[identifier]=com.x&include=bundleIdCapabilities"
        );
    }

    #[test]
    fn statuses_map_to_codes() {
        let scope = br#"{"errors":[{"code":"FORBIDDEN.REQUEST_DOES_NOT_MATCH_SCOPE","detail":"no"}]}"#;
        assert_eq!(map_status(403, scope, "/v1/devices").code(), codes::account::ASC_SCOPE);
        assert_eq!(map_status(403, b"{}", "/v1/devices").code(), codes::account::ASC_FORBIDDEN);
        for agreement in ["FORBIDDEN_ERROR.PLA_NOT_ACCEPTED", "FORBIDDEN.REQUIRED_AGREEMENTS_MISSING_OR_EXPIRED"] {
            let body = format!(r#"{{"errors":[{{"code":"{agreement}","detail":"agree first"}}]}}"#);
            let e = map_status(403, body.as_bytes(), "/v1/devices");
            assert_eq!(e.code(), codes::account::ASC_FORBIDDEN);
            assert!(e.hint().is_some_and(|h| h.contains("License Agreement")), "{agreement}");
        }
        assert!(!map_status(403, b"{}", "/v1/devices").hint().is_some_and(|h| h.contains("License Agreement")));
        assert_eq!(map_status(401, b"", "/v1/devices").code(), codes::account::ASC_UNAUTHORIZED);
        assert_eq!(map_status(404, b"", "/v1/devices").code(), codes::core::NOT_FOUND);
        assert_eq!(map_status(429, b"", "/v1/devices").code(), codes::account::ASC_RATE_LIMITED);
        assert_eq!(map_status(503, b"", "/v1/devices").code(), codes::account::APPLE_SERVICE_UNAVAILABLE);
        assert!(map_status(401, b"", "/v1/devices?filter[udid]=x").message().contains("/v1/devices"));
        assert!(!map_status(401, b"", "/v1/devices?filter[udid]=SECRET").message().contains("SECRET"));
    }
}
