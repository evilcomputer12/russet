// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Prefixed ULID identifiers (ARCHITECTURE §7.3): `job_…`, `ses_…`, `lse_…`, `sub_…`, `apr_…`.
//!
//! A ULID is 48 bits of milliseconds since the Unix epoch plus 80 random bits, written as 26 characters of
//! Crockford base32 (`0-9A-Z` without `I L O U`), so ids sort by creation time. Within one millisecond the
//! generator increments the random part, so ids of one process are strictly increasing. Newtype structs
//! serialize as their inner string with serde and ts-rs; the JSON Schema carries the pattern.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, codes};

/// Crockford's base32 alphabet.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The last ULID this process generated, for monotonic ids within one millisecond.
static LAST: Mutex<(u64, u128)> = Mutex::new((0, 0));

/// A fresh ULID (26 characters).
pub fn ulid() -> String {
    let now_ms = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis())
        .unwrap_or(u64::MAX)
        & 0xFFFF_FFFF_FFFF;
    let mut random = [0u8; 10];
    crate::time::os_fill(&mut random);
    let fresh = random.iter().fold(0u128, |acc, b| (acc << 8) | u128::from(*b));
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    let (ms, rand) = if now_ms > last.0 {
        (now_ms, fresh)
    } else {
        // Same (or an earlier, after a clock step) millisecond: keep increasing.
        let next = last.1 + 1;
        if next >> 80 == 0 { (last.0, next) } else { (last.0 + 1, fresh) }
    };
    *last = (ms, rand);
    drop(last);
    encode((u128::from(ms) << 80) | rand)
}

fn encode(value: u128) -> String {
    let mut out = [0u8; 26];
    // The first character carries the top 3 bits (26 × 5 = 130 bits).
    for (slot, shift) in out.iter_mut().zip((0..26u32).rev().map(|k| 5 * k)) {
        *slot = ALPHABET[((value >> shift) & 0x1F) as usize];
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether `s` is a canonical ULID: 26 upper-case Crockford characters, the first at most `7`.
pub fn is_ulid(s: &str) -> bool {
    s.len() == 26 && s.bytes().all(|b| ALPHABET.contains(&b)) && s.as_bytes()[0] <= b'7'
}

/// The milliseconds since the Unix epoch encoded in a canonical ULID.
pub fn ulid_millis(s: &str) -> Option<u64> {
    if !is_ulid(s) {
        return None;
    }
    let mut value: u128 = 0;
    for b in s.bytes() {
        let digit = ALPHABET.iter().position(|a| *a == b)? as u128;
        value = (value << 5) | digit;
    }
    u64::try_from(value >> 80).ok()
}

macro_rules! prefixed_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS))]
        pub struct $name(String);

        impl $name {
            /// The wire prefix, including the underscore.
            pub const PREFIX: &'static str = $prefix;

            /// A fresh id: the prefix plus a ULID.
            pub fn generate() -> Self {
                Self(format!("{}{}", $prefix, ulid()))
            }

            /// Parses and validates an id received from the wire (`core.invalidArgument` otherwise).
            pub fn parse(s: &str) -> crate::Result<Self> {
                match s.strip_prefix($prefix) {
                    Some(rest) if is_ulid(rest) => Ok(Self(s.to_owned())),
                    _ => Err(Error::new(
                        codes::core::INVALID_ARGUMENT,
                        format!("`{s}` is not a valid id: expected {}<26-character ULID>", $prefix),
                    )
                    .with_hint(concat!("Ids look like ", $prefix, "01J9Z3…; copy them from the list commands."))),
                }
            }

            /// The id as sent on the wire.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// When the id was generated (from its ULID), if it is well-formed.
            pub fn created_millis(&self) -> Option<u64> {
                self.0.strip_prefix($prefix).and_then(ulid_millis)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> crate::Result<Self> {
                Self::parse(s)
            }
        }

        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_g: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({
                    "type": "string",
                    "pattern": concat!("^", $prefix, "[0-9A-HJKMNP-TV-Z]{26}$"),
                })
            }
        }
    };
}

prefixed_id!(
    /// A job (`job_…`, ARCHITECTURE §6.2).
    JobId,
    "job_"
);
prefixed_id!(
    /// A run, debug or Flutter session (`ses_…`).
    SessionId,
    "ses_"
);
prefixed_id!(
    /// A lease (`lse_…`, ARCHITECTURE §6.3).
    LeaseId,
    "lse_"
);
prefixed_id!(
    /// A subscription (`sub_…`, ARCHITECTURE §6.4).
    SubscriptionId,
    "sub_"
);
prefixed_id!(
    /// An approval (`apr_…`, `^apr_[0-9A-Z]{26}$`, ARCHITECTURE §6.5).
    ApprovalId,
    "apr_"
);

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn generated_ids_have_the_prefix_and_parse_back() {
        let id = JobId::generate();
        assert!(id.as_str().starts_with("job_"));
        assert_eq!(id.as_str().len(), 4 + 26);
        assert_eq!(JobId::parse(id.as_str()).ok(), Some(id.clone()));
        let apr = ApprovalId::generate();
        let rest = apr.as_str().strip_prefix("apr_").expect("prefix");
        assert!(rest.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase()), "{apr}");
    }

    #[test]
    fn parse_rejects_wrong_prefixes_and_bodies() {
        assert!(JobId::parse("ses_01J00000000000000000000000").is_err());
        assert!(JobId::parse("job_01J0000000000000000000000").is_err(), "25 characters");
        assert!(JobId::parse("job_01J0000000000000000000000I").is_err(), "I is not Crockford");
        assert!(JobId::parse("job_81J00000000000000000000000").is_err(), "overflows 128 bits");
        assert!(JobId::parse("job_01j00000000000000000000000").is_err(), "lower case is not canonical");
        let e = JobId::parse("nope").expect_err("invalid");
        assert_eq!(e.code(), codes::core::INVALID_ARGUMENT);
    }

    #[test]
    fn ids_are_unique_and_increasing_within_a_process() {
        let ids: Vec<String> = (0..2000).map(|_| ulid()).collect();
        let unique: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());
        for w in ids.windows(2) {
            assert!(w[0] < w[1], "{} !< {}", w[0], w[1]);
        }
    }

    #[test]
    fn the_timestamp_round_trips() {
        let before =
            u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_millis()).expect("fits");
        let id = SessionId::generate();
        let ms = id.created_millis().expect("well-formed");
        assert!(ms >= before && ms <= before + 60_000, "{ms} vs {before}");
        assert_eq!(encode(0), "00000000000000000000000000");
        assert_eq!(encode(u128::MAX), "7ZZZZZZZZZZZZZZZZZZZZZZZZZ");
    }

    #[test]
    fn ids_serialize_as_strings_with_a_pattern_schema() {
        let id = LeaseId::parse("lse_01J00000000000000000000000").expect("valid");
        assert_eq!(serde_json::to_value(&id).expect("serializes"), serde_json::json!("lse_01J00000000000000000000000"));
        let schema = schemars::schema_for!(LeaseId).to_value();
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["pattern"], "^lse_[0-9A-HJKMNP-TV-Z]{26}$");
    }
}
