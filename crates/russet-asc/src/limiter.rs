// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The per-key rate limiter (SIGN §5.3): a token bucket of 3,600 requests per hour per key [V2] that also
//! follows the quota App Store Connect reports in `X-Rate-Limit`. Every client of one key shares one limiter
//! (russet-account keeps one per account), so the quota spans calls instead of starting afresh with each
//! client.

use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use crate::client::Quota;

/// App Store Connect's documented limit: 3,600 requests per hour per key [V2].
pub const REQUESTS_PER_HOUR: u32 = 3600;
/// Requests a key may send back to back before the hourly rate applies.
pub const BURST: u32 = 100;

#[derive(Debug)]
struct State {
    tokens: f64,
    last: Instant,
    quota: Quota,
}

/// A token bucket shared by every client of one App Store Connect key.
#[derive(Debug)]
pub struct RateLimiter {
    per_second: f64,
    burst: f64,
    state: Mutex<State>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::per_key()
    }
}

impl RateLimiter {
    /// 3,600 requests per hour with bursts of up to 100.
    pub fn per_key() -> Self {
        Self::new(REQUESTS_PER_HOUR, BURST)
    }

    /// `per_hour` requests per hour, `burst` of them back to back (both at least 1).
    pub fn new(per_hour: u32, burst: u32) -> Self {
        let burst = f64::from(burst.max(1));
        Self {
            per_second: f64::from(per_hour.max(1)) / 3600.0,
            burst,
            state: Mutex::new(State { tokens: burst, last: Instant::now(), quota: Quota::default() }),
        }
    }

    fn refill(&self, s: &mut State) {
        let now = Instant::now();
        let earned = now.saturating_duration_since(s.last).as_secs_f64() * self.per_second;
        s.tokens = (s.tokens + earned).min(self.burst);
        s.last = now;
    }

    /// Takes one request's token and returns how long to wait before sending it (zero while the bucket holds
    /// tokens). Concurrent callers queue behind each other: each reservation moves the bucket further into
    /// debt.
    pub fn reserve(&self) -> Duration {
        let Ok(mut s) = self.state.lock() else { return Duration::ZERO };
        self.refill(&mut s);
        s.tokens -= 1.0;
        if s.tokens >= 0.0 { Duration::ZERO } else { Duration::from_secs_f64(-s.tokens / self.per_second) }
    }

    /// Waits for a token.
    pub async fn acquire(&self) {
        let wait = self.reserve();
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Records the quota App Store Connect reported: the bucket never holds more than the requests left this
    /// hour (another tool may have used the same key).
    pub fn observe(&self, q: Quota) {
        let Ok(mut s) = self.state.lock() else { return };
        self.refill(&mut s);
        if let Some(rem) = q.remaining {
            s.tokens = s.tokens.min(f64::from(rem));
        }
        s.quota = q;
    }

    /// The last quota App Store Connect reported.
    pub fn quota(&self) -> Quota {
        self.state.lock().map(|s| s.quota).unwrap_or_default()
    }

    /// Tokens available now (negative while callers wait).
    pub fn available(&self) -> f64 {
        let Ok(mut s) = self.state.lock() else { return 0.0 };
        self.refill(&mut s);
        s.tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bucket_bursts_then_paces() {
        let l = RateLimiter::new(3600, 2);
        assert_eq!(l.reserve(), Duration::ZERO);
        assert_eq!(l.reserve(), Duration::ZERO);
        let wait = l.reserve();
        assert!(wait > Duration::from_millis(900) && wait <= Duration::from_secs(1), "{wait:?}");
        let next = l.reserve();
        assert!(next > Duration::from_millis(1900), "callers queue: {next:?}");
    }

    #[test]
    fn the_reported_quota_caps_the_bucket() {
        let l = RateLimiter::per_key();
        assert!(l.available() > 99.0);
        l.observe(Quota { limit: Some(3600), remaining: Some(0) });
        assert_eq!(l.quota().remaining, Some(0));
        assert!(l.reserve() > Duration::from_millis(900), "no request left this hour: wait for the next token");
    }
}
