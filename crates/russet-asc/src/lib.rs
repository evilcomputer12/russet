// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The async App Store Connect client (ARCHITECTURE §5.3; SIGN §5–§6): ES256 JWT, scoped GET tokens, a rate
//! limiter, open enums and a pluggable [`Auth`] (JWT here, DS2 in [`ds2`], W22).
//!
//! Wave 1 is read-only (SA-02 GET-only): the client sends nothing but GETs, and by default every request
//! carries a token scoped to exactly that request ([`TokenMode::PerRequest`]), so App Store Connect itself
//! refuses anything else [V3]. Mutations (W12) go through an approved provisioning plan.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::Arc;

use async_trait::async_trait;
use russet_doctor::{Category, Check, CheckCx, CheckResult, Requires, Status};

mod api;
mod client;
pub mod ds2;
mod jwt;
mod limiter;
pub mod model;
mod token;

pub use api::{
    AppsApi, BuildsApi, BundleIdsApi, CertificateFilter, CertificatesApi, CiApi, DeviceFilter, DevicesApi, PAGE_LIMIT,
    ProfileFilter, ProfilesApi, TestFlightApi,
};
pub use client::{
    ApiFlavor, AscClient, Auth, JwtAuth, Quota, RetryPolicy, TokenMode, is_loopback, map_status, offline,
    parse_rate_limit, path_and_query,
};
pub use jwt::{AscKey, Issuer, b64url_decode, unverified_claims};
pub use limiter::{BURST, REQUESTS_PER_HOUR, RateLimiter};
pub use token::{BEARER_LIFETIME, REFRESH_MARGIN, ScopeEntry, SystemClock, TokenFactory};
/// The URL type of [`JwtAuth::new`] and [`Auth::base_url`].
pub use url::Url;

/// The public App Store Connect API root.
pub const DEFAULT_BASE_URL: &str = "https://api.appstoreconnect.apple.com/";

/// App Store Connect doctor checks, for russetd's doctor seam. The check reads the App Store Connect root the
/// daemon uses: `asc` of `RUSSET_ENDPOINTS_FILE` (T3), else the public API.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    vec![Arc::new(AscReachable::from_env())]
}

/// The App Store Connect root of `RUSSET_ENDPOINTS_FILE` ([`asc_base_from`]), else [`DEFAULT_BASE_URL`].
pub fn asc_base_from_env() -> Result<Url, String> {
    let file = std::env::var_os("RUSSET_ENDPOINTS_FILE").filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    asc_base_from(file.as_deref())
}

/// The App Store Connect root of an endpoints file: a JSON object such as `{"asc": "http://127.0.0.1:8080/"}`, the
/// format of `RUSSET_ENDPOINTS_FILE` (T3). Without a file, or without `asc` in it, [`DEFAULT_BASE_URL`].
pub fn asc_base_from(file: Option<&std::path::Path>) -> Result<Url, String> {
    let default = || Url::parse(DEFAULT_BASE_URL).map_err(|e| e.to_string());
    let Some(path) = file else { return default() };
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    match value.get("asc") {
        None => default(),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| format!("{}: asc must be a URL string", path.display()))?;
            Url::parse(s).map_err(|e| format!("{}: asc `{s}` is not a URL: {e}", path.display()))
        }
    }
}

/// `asc.reachable` (`--online` only): an unauthenticated GET must get an HTTP answer (401 is the expected one);
/// a transport error means Russet cannot provision or verify accounts.
#[derive(Debug, Clone)]
pub struct AscReachable {
    /// The API root; an unusable `RUSSET_ENDPOINTS_FILE` is reported by the check.
    pub base: Result<Url, String>,
}

impl AscReachable {
    /// The check against `base`.
    pub fn new(base: Url) -> Self {
        Self { base: Ok(base) }
    }

    /// The check against the root the environment selects ([`asc_base_from_env`]).
    pub fn from_env() -> Self {
        Self { base: asc_base_from_env() }
    }
}

#[async_trait]
impl Check for AscReachable {
    fn id(&self) -> &'static str {
        "asc.reachable"
    }
    fn title(&self) -> &'static str {
        "App Store Connect is reachable"
    }
    fn category(&self) -> Category {
        Category::Network
    }
    fn requires(&self) -> Requires {
        Requires { network: true, ..Requires::default() }
    }
    async fn run(&self, cx: &CheckCx) -> CheckResult {
        if !cx.online {
            return CheckResult {
                status: Status::Skip,
                summary: "skipped (run with --online)".into(),
                detail: None,
                hint: None,
            };
        }
        let base = match &self.base {
            Ok(b) => b.clone(),
            Err(why) => {
                return CheckResult {
                    status: Status::Fail,
                    summary: "RUSSET_ENDPOINTS_FILE is unusable".into(),
                    detail: Some(why.clone()),
                    hint: Some("Fix or unset RUSSET_ENDPOINTS_FILE (a JSON object such as {\"asc\": \"http://127.0.0.1:8080/\"}).".into()),
                };
            }
        };
        if offline() && !is_loopback(&base) {
            return CheckResult {
                status: Status::Skip,
                summary: "skipped (RUSSET_OFFLINE=1 and App Store Connect is not a loopback host)".into(),
                detail: None,
                hint: None,
            };
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = match reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build() {
            Ok(c) => c,
            Err(e) => {
                return CheckResult {
                    status: Status::Fail,
                    summary: format!("no HTTP client: {e}"),
                    detail: None,
                    hint: None,
                };
            }
        };
        let Ok(url) = base.join("v1/apps") else {
            return CheckResult { status: Status::Fail, summary: "invalid API root".into(), detail: None, hint: None };
        };
        match client.get(url).send().await {
            Ok(r) => CheckResult {
                status: Status::Pass,
                summary: format!("App Store Connect answered (HTTP {})", r.status().as_u16()),
                detail: None,
                hint: None,
            },
            Err(e) => CheckResult {
                status: Status::Warn,
                summary: "App Store Connect is not reachable".into(),
                detail: Some(if e.is_timeout() { "timed out".into() } else { "connection failed".into() }),
                hint: Some(
                    "Check the network and proxy settings; provisioning and `russet account verify` need it.".into(),
                ),
            },
        }
    }
}
