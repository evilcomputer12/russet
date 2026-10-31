// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Time and randomness (testability contract T2): the [`Clock`] and [`Entropy`] seams with their real
//! implementations, plus the RFC 3339 helpers every wire timestamp uses (ARCHITECTURE §7.3: UTC strings).
//! `ManualClock` and `SeededEntropy` live in `russet-testkit`.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Wall-clock and monotonic time.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn instant(&self) -> Instant;
}

/// Randomness source.
pub trait Entropy: Send + Sync {
    fn fill(&self, b: &mut [u8]);
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
    fn instant(&self) -> Instant {
        Instant::now()
    }
}

/// The operating system's CSPRNG (`getrandom`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn fill(&self, b: &mut [u8]) {
        os_fill(b);
    }
}

/// Fills `b` from the operating system's CSPRNG. A failing CSPRNG is unrecoverable for a program that issues
/// tokens, so this panics instead of returning predictable bytes.
#[allow(clippy::panic)]
pub fn os_fill(b: &mut [u8]) {
    if let Err(e) = getrandom::fill(b) {
        panic!("the operating system's random number generator failed: {e}");
    }
}

/// `t` as an RFC 3339 UTC string with milliseconds, for example `2026-10-11T09:00:00.123Z`.
pub fn rfc3339(t: SystemTime) -> String {
    let dt = OffsetDateTime::from(t);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.millisecond()
    )
}

/// The current time as [`rfc3339`].
pub fn now_rfc3339() -> String {
    rfc3339(SystemTime::now())
}

/// Parses an RFC 3339 timestamp (any offset).
pub fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok().map(SystemTime::from)
}

/// Milliseconds since the Unix epoch (0 before it).
pub fn unix_millis(t: SystemTime) -> u64 {
    u64::try_from(t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()).unwrap_or(u64::MAX)
}

/// A duration in whole milliseconds, saturating (`…Ms` fields, ARCHITECTURE §7.3).
pub fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Parses a human duration: `30m`, `10s`, `2s`, `1h30m`, `500ms`, `7d`, or a bare number of seconds.
/// `0` is a zero duration (for example "never idle out" where a caller gives it that meaning).
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(secs) = s.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let mut total = Duration::ZERO;
    let mut rest = s;
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        if digits == 0 {
            return None;
        }
        let n: u64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit_len = rest.find(|c: char| c.is_ascii_digit()).unwrap_or(rest.len());
        let unit = &rest[..unit_len];
        rest = &rest[unit_len..];
        let d = match unit {
            "ms" => Duration::from_millis(n),
            "s" => Duration::from_secs(n),
            "m" => Duration::from_secs(n.checked_mul(60)?),
            "h" => Duration::from_secs(n.checked_mul(3600)?),
            "d" => Duration::from_secs(n.checked_mul(86_400)?),
            _ => return None,
        };
        total = total.checked_add(d)?;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_has_milliseconds_and_utc() {
        let t = UNIX_EPOCH + Duration::from_millis(1_791_709_200_123);
        assert_eq!(rfc3339(t), "2026-10-11T09:00:00.123Z");
        assert_eq!(parse_rfc3339("2026-10-11T09:00:00.123Z"), Some(t));
        assert_eq!(parse_rfc3339("2026-10-11T11:00:00.123+02:00"), Some(t));
        assert_eq!(parse_rfc3339("yesterday"), None);
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(1800)));
        assert_eq!(parse_duration("2s"), Some(Duration::from_secs(2)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("7d"), Some(Duration::from_secs(7 * 86_400)));
        assert_eq!(parse_duration("15"), Some(Duration::from_secs(15)));
        assert_eq!(parse_duration("0"), Some(Duration::ZERO));
        assert_eq!(parse_duration("5 minutes"), None);
        assert_eq!(parse_duration("m"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn entropy_fills_and_differs() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        OsEntropy.fill(&mut a);
        OsEntropy.fill(&mut b);
        assert_ne!(a, b);
        assert!(SystemClock.now() > UNIX_EPOCH);
    }
}
