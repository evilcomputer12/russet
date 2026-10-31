// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Value types shared by the configuration files: durations (`"30m"`) and byte sizes (`"5GiB"`).
//!
//! Both are written as strings in TOML (ARCHITECTURE §14.3) and keep that form on the wire. Parsing happens at
//! load time, so a bad value is reported with its file, line and column like every other invalid value.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A duration written as `"30m"`, `"2s"`, `"1h30m"`, `"500ms"`, `"7d"` or `"0"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationStr(pub Duration);

impl DurationStr {
    /// `n` seconds.
    pub const fn secs(n: u64) -> Self {
        Self(Duration::from_secs(n))
    }

    /// `n` minutes.
    pub const fn mins(n: u64) -> Self {
        Self(Duration::from_secs(n * 60))
    }

    /// The duration.
    pub const fn get(self) -> Duration {
        self.0
    }
}

const DURATION_UNITS: &[(&str, u128)] = &[("d", 86_400_000), ("h", 3_600_000), ("m", 60_000), ("s", 1_000), ("ms", 1)];

impl FromStr for DurationStr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        if t == "0" {
            return Ok(Self(Duration::ZERO));
        }
        if t.is_empty() {
            return Err("empty duration (write for example \"30m\", \"2s\" or \"0\")".into());
        }
        let mut total_ms: u128 = 0;
        let mut rest = t;
        while !rest.is_empty() {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return Err(format!("invalid duration `{s}`: expected a number before each unit (\"1h30m\")"));
            }
            let (num, tail) = rest.split_at(digits);
            let unit_len = tail.bytes().take_while(u8::is_ascii_alphabetic).count();
            let (unit, tail) = tail.split_at(unit_len);
            let factor = DURATION_UNITS
                .iter()
                .find(|(u, _)| *u == unit)
                .map(|(_, f)| *f)
                .ok_or_else(|| format!("invalid duration `{s}`: unknown unit `{unit}` (use ms, s, m, h or d)"))?;
            let n: u128 = num.parse().map_err(|_| format!("invalid duration `{s}`: number too large"))?;
            total_ms = n
                .checked_mul(factor)
                .and_then(|v| total_ms.checked_add(v))
                .ok_or_else(|| format!("invalid duration `{s}`: too large"))?;
            rest = tail;
        }
        let ms = u64::try_from(total_ms).map_err(|_| format!("invalid duration `{s}`: too large"))?;
        Ok(Self(Duration::from_millis(ms)))
    }
}

impl fmt::Display for DurationStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ms = self.0.as_millis();
        if ms == 0 {
            return f.write_str("0");
        }
        for (unit, factor) in DURATION_UNITS {
            if ms >= *factor {
                let n = ms / factor;
                ms %= factor;
                write!(f, "{n}{unit}")?;
            }
        }
        Ok(())
    }
}

impl Serialize for DurationStr {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for DurationStr {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for DurationStr {
    fn schema_name() -> Cow<'static, str> {
        "Duration".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "A duration such as \"30m\", \"2s\", \"1h30m\", \"500ms\" or \"0\".",
            "pattern": "^(0|([0-9]+(ms|s|m|h|d))+)$"
        })
    }
}

/// A byte size written as `"5GiB"`, `"512MiB"`, `"20GB"` or `"1024"` (bytes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteSize(pub u64);

impl ByteSize {
    /// `n` GiB.
    pub const fn gib(n: u64) -> Self {
        Self(n << 30)
    }

    /// The size in bytes.
    pub const fn bytes(self) -> u64 {
        self.0
    }
}

const SIZE_UNITS: &[(&str, u64)] = &[
    ("TiB", 1 << 40),
    ("GiB", 1 << 30),
    ("MiB", 1 << 20),
    ("KiB", 1 << 10),
    ("TB", 1_000_000_000_000),
    ("GB", 1_000_000_000),
    ("MB", 1_000_000),
    ("KB", 1_000),
    ("kB", 1_000),
    ("B", 1),
];

impl FromStr for ByteSize {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return Err(format!("invalid size `{s}`: write for example \"5GiB\", \"512MiB\" or \"1024\""));
        }
        let (num, unit) = t.split_at(digits);
        let unit = unit.trim();
        let n: u64 = num.parse().map_err(|_| format!("invalid size `{s}`: number too large"))?;
        let factor = if unit.is_empty() {
            1
        } else {
            SIZE_UNITS
                .iter()
                .find(|(u, _)| *u == unit)
                .map(|(_, f)| *f)
                .ok_or_else(|| format!("invalid size `{s}`: unknown unit `{unit}` (use B, KiB, MiB, GiB or TiB)"))?
        };
        n.checked_mul(factor).map(Self).ok_or_else(|| format!("invalid size `{s}`: too large"))
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The exact unit with the smallest number: 5GiB stays "5GiB", 20000000000 becomes "20GB".
        let best = SIZE_UNITS
            .iter()
            .filter(|(unit, factor)| {
                *unit != "kB" && *factor > 1 && self.0 >= *factor && self.0.is_multiple_of(*factor)
            })
            .min_by_key(|(_, factor)| self.0 / factor);
        match best {
            Some((unit, factor)) => write!(f, "{}{unit}", self.0 / factor),
            None => write!(f, "{}", self.0),
        }
    }
}

impl Serialize for ByteSize {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ByteSize {
    fn schema_name() -> Cow<'static, str> {
        "ByteSize".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "A size such as \"5GiB\", \"512MiB\", \"20GB\" or \"1024\" (bytes).",
            "pattern": "^[0-9]+ ?(B|KiB|MiB|GiB|TiB|kB|KB|MB|GB|TB)?$"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_and_print_canonically() {
        let cases = [("30m", 1800), ("2s", 2), ("1h30m", 5400), ("0", 0), ("7d", 604_800)];
        for (text, secs) in cases {
            let d: DurationStr = text.parse().expect(text);
            assert_eq!(d.get().as_secs(), secs, "{text}");
            assert_eq!(d.to_string(), text);
        }
        let ms: DurationStr = "500ms".parse().expect("500ms");
        assert_eq!(ms.get().as_millis(), 500);
        assert_eq!("90m".parse::<DurationStr>().map(|d| d.to_string()), Ok("1h30m".to_owned()));
        for bad in ["", "thirty", "30", "5x", "m5"] {
            assert!(bad.parse::<DurationStr>().is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn sizes_parse_and_print_canonically() {
        assert_eq!("5GiB".parse::<ByteSize>(), Ok(ByteSize::gib(5)));
        assert_eq!("20GiB".parse::<ByteSize>().map(|b| b.to_string()), Ok("20GiB".to_owned()));
        assert_eq!("1024".parse::<ByteSize>().map(|b| b.to_string()), Ok("1KiB".to_owned()));
        assert_eq!("20GB".parse::<ByteSize>(), Ok(ByteSize(20_000_000_000)));
        assert_eq!(ByteSize(20_000_000_000).to_string(), "20GB");
        assert_eq!(ByteSize(1000).to_string(), "1KB");
        assert_eq!(ByteSize(1023).to_string(), "1023");
        assert_eq!("512 MiB".parse::<ByteSize>(), Ok(ByteSize(512 << 20)));
        for bad in ["", "GiB", "5 gigs", "-1"] {
            assert!(bad.parse::<ByteSize>().is_err(), "{bad} should not parse");
        }
    }
}
