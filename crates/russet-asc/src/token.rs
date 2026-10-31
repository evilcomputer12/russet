// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Token minting and caching (SIGN §5.2): unscoped bearer tokens live 19 minutes and are reused until 60 seconds
//! remain; scoped GET tokens are minted per call, for agents and read-only work.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use russet_core::Result;
use russet_core::time::Clock;
use secrecy::{ExposeSecret as _, SecretString};

use crate::jwt::AscKey;

/// The bearer token lifetime: App Store Connect rejects tokens over 20 minutes.
pub const BEARER_LIFETIME: Duration = Duration::from_secs(19 * 60);
/// A cached bearer token is replaced when less than this remains.
pub const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// One entry of a scoped token, for example `GET /v1/apps?filter[platform]=IOS` (method, path and the
/// optional query; App Store Connect ignores `limit`, `cursor` and `sort` when matching).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeEntry(pub String);

impl ScopeEntry {
    /// The GET scope of a path and query. `limit`, `cursor` and `sort` are dropped, so one scope covers every
    /// page of a listing.
    pub fn get(path_and_query: &str) -> Self {
        let (path, query) = path_and_query.split_once('?').unwrap_or((path_and_query, ""));
        let kept: Vec<&str> = query
            .split('&')
            .filter(|p| !p.is_empty())
            .filter(|p| {
                let name = p.split('=').next().unwrap_or("");
                !matches!(name, "limit" | "cursor" | "sort")
            })
            .collect();
        if kept.is_empty() { Self(format!("GET {path}")) } else { Self(format!("GET {path}?{}", kept.join("&"))) }
    }

    /// Whether the entry is a GET (Wave 1 mints nothing else).
    pub fn is_get(&self) -> bool {
        self.0.starts_with("GET ")
    }
}

/// The wall clock and a monotonic clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
    fn instant(&self) -> Instant {
        Instant::now()
    }
}

/// Mints bearer tokens.
pub struct TokenFactory {
    key: AscKey,
    clock: Arc<dyn Clock>,
    cached: Mutex<Option<(SecretString, Instant)>>,
}

impl std::fmt::Debug for TokenFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenFactory").field("key", &self.key).finish_non_exhaustive()
    }
}

impl TokenFactory {
    pub fn new(key: AscKey) -> Self {
        Self::with_clock(key, Arc::new(SystemClock))
    }

    /// A factory on an injected clock (tests).
    pub fn with_clock(key: AscKey, clock: Arc<dyn Clock>) -> Self {
        Self { key, clock, cached: Mutex::new(None) }
    }

    pub fn key(&self) -> &AscKey {
        &self.key
    }

    /// A cached bearer token (at most 19 minutes old; replaced when less than a minute remains).
    pub fn bearer(&self) -> Result<SecretString> {
        let now = self.clock.instant();
        let mut cache = self.cached.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((token, expires)) = cache.as_ref()
            && expires.saturating_duration_since(now) > REFRESH_MARGIN
        {
            return Ok(SecretString::from(token.expose_secret().to_owned()));
        }
        let token = self.key.mint(self.clock.now(), BEARER_LIFETIME, None)?;
        *cache = Some((SecretString::from(token.expose_secret().to_owned()), now + BEARER_LIFETIME));
        Ok(token)
    }

    /// A GET-scoped token for agents and read-only work. The lifetime is not capped here: App Store Connect
    /// accepts long-lived scoped GET tokens only for some resources and answers 401 otherwise.
    pub fn scoped(&self, scope: &[ScopeEntry], lifetime: Duration) -> Result<SecretString> {
        if scope.is_empty() || scope.iter().any(|s| !s.is_get()) {
            return Err(russet_core::Error::new(
                russet_core::error::codes::core::INVALID_ARGUMENT,
                "scoped tokens are minted for GET requests only",
            )
            .with_hint("Use one `GET /v1/...` entry per request; mutations need an approved plan (W12)."));
        }
        let entries: Vec<String> = scope.iter().map(|s| s.0.clone()).collect();
        self.key.mint(self.clock.now(), lifetime, Some(&entries))
    }
}
