// SPDX-License-Identifier: GPL-3.0-or-later
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
pub mod model;
mod token;

pub use api::{
    AppsApi, BuildsApi, BundleIdsApi, CertificateFilter, CertificatesApi, CiApi, DeviceFilter, DevicesApi, PAGE_LIMIT,
    ProfileFilter, ProfilesApi, TestFlightApi,
};
pub use client::{
    ApiFlavor, AscClient, Auth, JwtAuth, Quota, RetryPolicy, TokenMode, map_status, parse_rate_limit, path_and_query,
};
pub use jwt::{AscKey, Issuer, b64url_decode, unverified_claims};
pub use token::{BEARER_LIFETIME, REFRESH_MARGIN, ScopeEntry, SystemClock, TokenFactory};

/// The public App Store Connect API root.
pub const DEFAULT_BASE_URL: &str = "https://api.appstoreconnect.apple.com/";

/// App Store Connect doctor checks, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    vec![Arc::new(AscReachable { base: DEFAULT_BASE_URL.into() })]
}

/// `asc.reachable` (`--online` only): an unauthenticated GET must get an HTTP answer (401 is the expected one);
/// a transport error means Russet cannot provision or verify accounts.
#[derive(Debug, Clone)]
pub struct AscReachable {
    pub base: String,
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
        match client.get(format!("{}v1/apps", self.base)).send().await {
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
