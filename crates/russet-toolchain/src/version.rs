// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Loose numeric versions (`6.4`, `6.4.0`, `21.1.6`, `3.12.15`) and requirements (`6.4`, `>=23.1.3`, `=1.1.0`).
//! Missing components compare as zero, so `6.4 == 6.4.0`.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// A dotted numeric version; a suffix after the numbers (`6.4-dev`, `21.0.0git`) is ignored. Equality ignores
/// trailing zeros (`6.4 == 6.4.0`), like the ordering.
#[derive(Debug, Clone)]
pub struct Version {
    parts: Vec<u64>,
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl std::hash::Hash for Version {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let end = self.parts.iter().rposition(|p| *p != 0).map_or(0, |i| i + 1);
        self.parts[..end].hash(state);
    }
}

impl Version {
    /// A version from its numeric components (`[23, 1]` is 23.1); no components is `0`.
    pub fn new(mut parts: Vec<u64>) -> Self {
        if parts.is_empty() {
            parts.push(0);
        }
        Self { parts }
    }

    /// The numeric components.
    pub fn parts(&self) -> &[u64] {
        &self.parts
    }

    /// `major.minor` as a version.
    pub fn minor_version(&self) -> Version {
        Version { parts: self.parts.iter().copied().chain(std::iter::repeat(0)).take(2).collect() }
    }

    /// Whether `self` starts with every component of `prefix` (`6.4.1` matches `6.4`).
    pub fn matches_prefix(&self, prefix: &Version) -> bool {
        prefix.parts.iter().enumerate().all(|(i, p)| self.parts.get(i).copied().unwrap_or(0) == *p)
    }

    /// Finds the first version in `text` (for `--version` output).
    pub fn find_in(text: &str) -> Option<Version> {
        text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .filter(|t| t.contains('.') && t.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .find_map(|t| t.trim_end_matches('.').parse().ok())
    }
}

impl FromStr for Version {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().trim_start_matches('v');
        let numeric: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let parts: Result<Vec<u64>, _> =
            numeric.trim_end_matches('.').split('.').filter(|p| !p.is_empty()).map(str::parse).collect();
        match parts {
            Ok(p) if !p.is_empty() => Ok(Self { parts: p }),
            _ => Err(format!("`{s}` is not a version")),
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let n = self.parts.len().max(other.parts.len());
        (0..n)
            .map(|i| self.parts.get(i).copied().unwrap_or(0).cmp(&other.parts.get(i).copied().unwrap_or(0)))
            .find(|o| *o != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s: Vec<String> = self.parts.iter().map(u64::to_string).collect();
        f.write_str(&s.join("."))
    }
}

/// A version requirement: `6.4` (prefix: any 6.4.x), `=6.4.0` (exact), `>=23.1.3` (minimum).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionReq {
    Prefix(Version),
    Exact(Version),
    AtLeast(Version),
}

impl FromStr for VersionReq {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        if let Some(v) = t.strip_prefix(">=") {
            Ok(Self::AtLeast(v.parse()?))
        } else if let Some(v) = t.strip_prefix('=') {
            Ok(Self::Exact(v.parse()?))
        } else {
            Ok(Self::Prefix(t.parse()?))
        }
    }
}

impl VersionReq {
    /// Whether `v` satisfies the requirement.
    pub fn matches(&self, v: &Version) -> bool {
        match self {
            Self::Prefix(p) => v.matches_prefix(p),
            Self::Exact(e) => v == e,
            Self::AtLeast(min) => v >= min,
        }
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prefix(v) => write!(f, "{v}"),
            Self::Exact(v) => write!(f, "={v}"),
            Self::AtLeast(v) => write!(f, ">={v}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        s.parse().expect(s)
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(v("6.4"), v("6.4.0"));
        assert!(v("23.1.1") < v("23.1.3"));
        assert!(v("21.1.6") < v("23.1.3"));
        assert!(v("1.10.0") > v("1.9.9"));
        assert_eq!(v("v1.1.0").to_string(), "1.1.0");
        assert_eq!(v("6.4-dev").to_string(), "6.4");
        assert!("abc".parse::<Version>().is_err());
        assert_eq!(v("6.4.2").minor_version(), v("6.4"));
    }

    #[test]
    fn requirements_match() {
        let req: VersionReq = ">=23.1.3".parse().expect("req");
        assert!(req.matches(&v("23.1.3")) && req.matches(&v("24.0")) && !req.matches(&v("23.1.1")));
        let req: VersionReq = "6.4".parse().expect("req");
        assert!(req.matches(&v("6.4.0")) && req.matches(&v("6.4.1")) && !req.matches(&v("6.3")));
        let req: VersionReq = "=1.1.0".parse().expect("req");
        assert!(req.matches(&v("1.1")) && !req.matches(&v("1.1.1")));
    }

    #[test]
    fn versions_are_found_in_tool_output() {
        assert_eq!(Version::find_in("Swift version 6.4 (swift-6.4-RELEASE)"), Some(v("6.4")));
        assert_eq!(Version::find_in("LLD 21.1.6 (compatible with GNU linkers)"), Some(v("21.1.6")));
        assert_eq!(Version::find_in("lldb version 21.0.0 (https://github.com/x.git revision 9a)"), Some(v("21.0.0")));
        assert_eq!(Version::find_in("xtool 1.21.0"), Some(v("1.21.0")));
        assert_eq!(Version::find_in("no digits"), None);
    }
}
