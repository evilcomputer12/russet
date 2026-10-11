// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! HTTP endpoints (testability contract T3). The defaults are Apple's, GitHub's and Flutter's production hosts;
//! `RUSSET_ENDPOINTS_FILE` (a JSON object with any of the camelCase keys `asc`, `gsa`, `tss`, `anisetteLibs`,
//! `github`, `flutterStorage`) overrides them for tests, and `RUSSET_OFFLINE=1` makes [`Endpoints::check`]
//! refuse every non-loopback host, so L1–L6 tests never leave the machine.

use std::path::Path;

use url::Url;

use crate::error::{Error, codes};

/// Base URLs of the external services Russet talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub asc: Url,
    pub gsa: Url,
    pub tss: Url,
    pub anisette_libs: Url,
    pub github: Url,
    pub flutter_storage: Url,
}

fn url(s: &str) -> Url {
    // The defaults are literals checked by `defaults_parse`; a failure here is a typo in this file.
    #[allow(clippy::expect_used)]
    Url::parse(s).expect("default endpoint URLs are valid")
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            asc: url("https://api.appstoreconnect.apple.com/"),
            gsa: url("https://gsa.apple.com/"),
            tss: url("https://gs.apple.com/"),
            anisette_libs: url("https://apps.mzstatic.com/"),
            github: url("https://api.github.com/"),
            flutter_storage: url("https://storage.googleapis.com/"),
        }
    }
}

impl Endpoints {
    /// The defaults, overridden by `RUSSET_ENDPOINTS_FILE` when it is set.
    pub fn from_env() -> crate::Result<Self> {
        match std::env::var_os("RUSSET_ENDPOINTS_FILE").filter(|v| !v.is_empty()) {
            Some(p) => Self::from_file(Path::new(&p)),
            None => Ok(Self::default()),
        }
    }

    /// The defaults with the keys of the JSON object in `path` replaced.
    pub fn from_file(path: &Path) -> crate::Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(format!("reading {}", path.display()), e))?;
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            Error::new(codes::core::INVALID_ARGUMENT, format!("{} is not valid JSON: {e}", path.display()))
                .with_hint("RUSSET_ENDPOINTS_FILE holds a JSON object such as {\"asc\": \"http://127.0.0.1:8080/\"}.")
        })?;
        let mut out = Self::default();
        let Some(map) = value.as_object() else {
            return Err(Error::invalid_argument(format!("{} must hold a JSON object", path.display())));
        };
        for (key, v) in map {
            let slot = match key.as_str() {
                "asc" => &mut out.asc,
                "gsa" => &mut out.gsa,
                "tss" => &mut out.tss,
                "anisetteLibs" => &mut out.anisette_libs,
                "github" => &mut out.github,
                "flutterStorage" => &mut out.flutter_storage,
                other => {
                    return Err(Error::invalid_argument(format!("{}: unknown endpoint `{other}`", path.display()))
                        .with_hint("Known keys: asc, gsa, tss, anisetteLibs, github, flutterStorage."));
                }
            };
            let s = v.as_str().ok_or_else(|| Error::invalid_argument(format!("{key} must be a URL string")))?;
            *slot = Url::parse(s).map_err(|e| Error::invalid_argument(format!("{key}: `{s}` is not a URL: {e}")))?;
        }
        Ok(out)
    }

    /// Whether `RUSSET_OFFLINE=1` is set.
    pub fn offline() -> bool {
        std::env::var_os("RUSSET_OFFLINE").is_some_and(|v| v == "1")
    }

    /// Refuses a request to `target` when offline mode is on and the host is not loopback (T3). HTTP clients call
    /// this before every request.
    pub fn check(target: &Url) -> crate::Result<()> {
        if !Self::offline() || is_loopback(target) {
            return Ok(());
        }
        Err(Error::new(
            codes::core::INVALID_ARGUMENT,
            format!("RUSSET_OFFLINE=1 refuses the non-loopback host {}", target.host_str().unwrap_or("?")),
        )
        .with_hint("Point the service at a local fake with RUSSET_ENDPOINTS_FILE, or unset RUSSET_OFFLINE."))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse() {
        let e = Endpoints::default();
        assert_eq!(e.asc.host_str(), Some("api.appstoreconnect.apple.com"));
        assert!(!is_loopback(&e.gsa));
    }

    #[test]
    fn a_file_overrides_some_endpoints() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("endpoints.json");
        std::fs::write(&p, r#"{"asc": "http://127.0.0.1:8080/", "anisetteLibs": "http://localhost:9/"}"#)
            .expect("write");
        let e = Endpoints::from_file(&p).expect("loads");
        assert_eq!(e.asc.as_str(), "http://127.0.0.1:8080/");
        assert!(is_loopback(&e.anisette_libs));
        assert_eq!(e.tss, Endpoints::default().tss);
        std::fs::write(&p, r#"{"nope": "http://x/"}"#).expect("write");
        assert!(Endpoints::from_file(&p).is_err());
    }
}
