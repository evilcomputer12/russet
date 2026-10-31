// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! TLS for `gsa.apple.com` (SIGN §7.2 item 6, [V15]): it chains to Apple Root CA, which is not in the system
//! stores. Russet ships only the certificate's SHA-256, downloads `AppleIncRootCertificate.cer` from apple.com on
//! first use, checks the pin and caches it; the GrandSlam client then trusts that one root and nothing else, and
//! never reuses connections (GSA dislikes them).

use std::path::{Path, PathBuf};
use std::time::Duration;

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::net::Endpoints;
use url::Url;

/// Where Apple publishes its root certificate.
pub const APPLE_ROOT_URL: &str = "https://www.apple.com/appleca/AppleIncRootCertificate.cer";

/// SHA-256 of Apple Inc. Root Certificate (`B0:B1:73:0E:…:01:F0:24`, valid to 2035-02-09) [V15].
pub const APPLE_ROOT_SHA256: [u8; 32] = [
    0xB0, 0xB1, 0x73, 0x0E, 0xCB, 0xC7, 0xFF, 0x45, 0x05, 0x14, 0x2C, 0x49, 0xF1, 0x29, 0x5E, 0x6E, 0xDA, 0x6B, 0xCA,
    0xED, 0x7E, 0x2C, 0x68, 0xC5, 0xBE, 0x91, 0xB5, 0xA1, 0x10, 0x01, 0xF0, 0x24,
];

/// The cache file under the data directory.
pub fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join("gsa").join("AppleIncRootCertificate.cer")
}

/// Apple Root CA, checked against [`APPLE_ROOT_SHA256`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleRoot {
    der: Vec<u8>,
}

impl AppleRoot {
    /// The certificate if `der` matches the pin.
    pub fn from_der(der: Vec<u8>) -> Result<Self> {
        let digest = ring::digest::digest(&ring::digest::SHA256, &der);
        if digest.as_ref() != APPLE_ROOT_SHA256 {
            return Err(Error::new(
                codes::gsa::ROOT_PIN_MISMATCH,
                "the downloaded Apple Root CA certificate does not match Russet's pinned SHA-256",
            )
            .with_hint("Something between this machine and apple.com altered the certificate. Russet will not sign in to Apple over this connection; try another network."));
        }
        Ok(Self { der })
    }

    pub fn der(&self) -> &[u8] {
        &self.der
    }

    /// The cached certificate (re-checked against the pin), else a download from [`APPLE_ROOT_URL`] that is
    /// checked and cached (0600).
    pub async fn ensure(data_dir: &Path) -> Result<Self> {
        let path = cache_path(data_dir);
        if let Ok(der) = std::fs::read(&path)
            && let Ok(root) = Self::from_der(der)
        {
            return Ok(root);
        }
        #[allow(clippy::unwrap_used)] // a constant, valid URL
        let url = Url::parse(APPLE_ROOT_URL).unwrap();
        Endpoints::check(&url)?;
        let http = crate::http::plain_client(Duration::from_secs(30))?;
        let resp = http.get(url).send().await.map_err(|e| {
            Error::new(codes::account::APPLE_SERVICE_UNAVAILABLE, format!("cannot download Apple Root CA: {e}"))
                .with_hint("Russet needs apple.com once to trust gsa.apple.com; check the network and try again.")
                .retryable(true)
        })?;
        if !resp.status().is_success() {
            return Err(Error::new(
                codes::account::APPLE_SERVICE_UNAVAILABLE,
                format!("apple.com answered {} for Apple Root CA", resp.status().as_u16()),
            )
            .retryable(true));
        }
        let der =
            resp.bytes().await.map_err(|e| Error::new(codes::account::APPLE_SERVICE_UNAVAILABLE, e.to_string()))?;
        let root = Self::from_der(der.to_vec())?;
        crate::http::write_private(&path, root.der())?;
        Ok(root)
    }
}

/// The HTTP client for GrandSlam at `base`: over `https` it trusts only Apple Root CA ([`AppleRoot::ensure`]);
/// a loopback `http` base (the mocked suites, `RUSSET_ENDPOINTS_FILE`) needs no certificate. Connections are
/// never reused.
pub async fn gsa_client(base: &Url, data_dir: &Path) -> Result<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut b = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none());
    if base.scheme() == "https" {
        let root = AppleRoot::ensure(data_dir).await?;
        let cert = reqwest::Certificate::from_der(root.der())
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("Apple Root CA is unusable: {e}")))?;
        b = b.tls_certs_only([cert]);
    } else if !russet_core::net::is_loopback(base) {
        return Err(Error::invalid_argument(format!("GrandSlam must be reached over https, not {}", base.scheme()))
            .with_hint("Only loopback test fakes may use http (RUSSET_ENDPOINTS_FILE)."));
    }
    b.build().map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot build the GrandSlam HTTP client: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrong_certificate_is_refused() {
        let e = AppleRoot::from_der(b"not apple".to_vec()).err();
        assert_eq!(e.map(|e| e.code()), Some(codes::gsa::ROOT_PIN_MISMATCH));
    }

    #[tokio::test]
    async fn loopback_http_needs_no_root_and_remote_http_is_refused() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let local = Url::parse("http://127.0.0.1:9/").expect("url");
        assert!(gsa_client(&local, tmp.path()).await.is_ok());
        let remote = Url::parse("http://gsa.example/").expect("url");
        assert!(gsa_client(&remote, tmp.path()).await.is_err());
        assert!(!cache_path(tmp.path()).exists());
    }
}
