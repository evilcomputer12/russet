// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Signing identities in memory: a certificate plus its private key (RSA, as Apple issues them, or ECDSA P-256
//! and Ed25519, which apple-codesign also signs with). russet-account seals the key and wraps this type.

use std::time::SystemTime;

use apple_codesign::cryptography::InMemoryPrivateKey;
use russet_core::Result;
use russet_core::error::{Error, codes};
use x509_certificate::{CapturedX509Certificate, KeyInfoSigner, Sign as _};
use zeroize::Zeroizing;

use crate::CodeSigningIdentity;
use crate::util::normalize_serial;

/// The public facts of a certificate that identities and planning need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateInfo {
    /// Uppercase hex without leading zeros (the portal's `serialNumber` form).
    pub serial: String,
    pub common_name: Option<String>,
    /// The subject's organizational unit: the team ID on Apple-issued certificates.
    pub team_id: Option<String>,
    pub not_before: SystemTime,
    pub not_after: SystemTime,
    /// Lower-case hex SHA-1 of the DER (what `security find-identity` shows).
    pub sha1: String,
    pub kind: CertificateKind,
}

/// The kind of signing certificate, from its common name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CertificateKind {
    /// `Apple Development: …` or `iPhone Developer: …`.
    Development,
    /// `Apple Distribution: …` or `iPhone Distribution: …`.
    Distribution,
    Unknown,
}

/// Parses the facts of a DER certificate.
pub fn certificate_info(der: &[u8]) -> Result<CertificateInfo> {
    let cert = CapturedX509Certificate::from_der(der.to_vec()).map_err(|e| {
        Error::new(codes::sign::NO_IDENTITY, format!("the signing certificate cannot be parsed: {e}"))
            .with_hint("Import the identity again (`russet account import --from-xtool` or a .p12).")
            .caused_by(e)
    })?;
    let common_name = cert.subject_common_name();
    let team_id = cert.subject_name().iter_organizational_unit().next().and_then(|a| a.to_string().ok());
    let kind = match common_name.as_deref() {
        Some(cn) if cn.starts_with("Apple Development") || cn.starts_with("iPhone Developer") => {
            CertificateKind::Development
        }
        Some(cn) if cn.starts_with("Apple Distribution") || cn.starts_with("iPhone Distribution") => {
            CertificateKind::Distribution
        }
        _ => CertificateKind::Unknown,
    };
    let sha1 = russet_macho::HashType::Sha1.digest(der).map(|d| crate::util::hex(&d)).unwrap_or_default();
    Ok(CertificateInfo {
        serial: normalize_serial(cert.serial_number_asn1().as_slice()),
        common_name,
        team_id,
        not_before: cert.validity_not_before().into(),
        not_after: cert.validity_not_after().into(),
        sha1,
        kind,
    })
}

/// A certificate and its private key, in memory. Implements [`CodeSigningIdentity`].
pub struct InMemoryIdentity {
    cert_der: Vec<u8>,
    team_id: String,
    key: InMemoryPrivateKey,
}

impl std::fmt::Debug for InMemoryIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryIdentity").field("key", &"***").finish_non_exhaustive()
    }
}

fn key_error(why: impl std::fmt::Display) -> Error {
    Error::new(codes::sign::NO_IDENTITY, format!("the signing key is unusable: {why}"))
        .with_hint("Import the identity again; the private key must belong to the certificate.")
}

impl InMemoryIdentity {
    /// Builds an identity from a DER certificate and a PKCS#8 DER private key. Fails unless the key belongs to
    /// the certificate. `team_id` defaults to the certificate's OU.
    pub fn from_pkcs8(cert_der: Vec<u8>, key_pkcs8_der: &[u8], team_id: Option<String>) -> Result<Self> {
        let key = InMemoryPrivateKey::from_pkcs8_der(key_pkcs8_der).map_err(key_error)?;
        Self::from_key(cert_der, key, team_id)
    }

    /// Builds an identity from a PEM private key: PKCS#8 (`PRIVATE KEY`) or PKCS#1 (`RSA PRIVATE KEY`, what
    /// xtool stores).
    pub fn from_pem(cert_der: Vec<u8>, key_pem: &[u8], team_id: Option<String>) -> Result<Self> {
        let text = std::str::from_utf8(key_pem).map_err(|_| key_error("the PEM is not text"))?;
        let (label, der) = decode_pem(text).ok_or_else(|| key_error("no PEM private key block"))?;
        let key = match label.as_str() {
            "PRIVATE KEY" => InMemoryPrivateKey::from_pkcs8_der(&*der).map_err(key_error)?,
            "RSA PRIVATE KEY" => InMemoryPrivateKey::from_pkcs1_der(&*der).map_err(key_error)?,
            other => return Err(key_error(format!("unsupported PEM block `{other}`"))),
        };
        Self::from_key(cert_der, key, team_id)
    }

    /// Builds an identity from PKCS#12 (`.p12`) data: the first private key with its certificate. p12-keystore
    /// reads PBES2 (AES with an HMAC-SHA-256 MAC, what OpenSSL 3 and recent Keychain Access write) and the legacy
    /// PBES1 forms (3DES, RC2, SHA-1 MAC). apple-codesign's own reader is not used: it handles only the legacy
    /// forms and asserts on a SHA-256 MAC in debug builds.
    pub fn from_p12(p12: &[u8], password: &str, team_id: Option<String>) -> Result<Self> {
        let unreadable = |why: String| {
            Error::new(codes::sign::NO_IDENTITY, format!("cannot read the .p12: {why}"))
                .with_hint("Check the password; the file must hold one certificate and its private key.")
        };
        let store = p12_keystore::KeyStore::from_pkcs12(p12, password, p12_keystore::Pkcs12ImportPolicy::Strict)
            .map_err(|e| unreadable(e.to_string()))?;
        let (_, chain) = store
            .private_key_chain()
            .ok_or_else(|| unreadable("it holds no private key with its certificate".into()))?;
        let cert = chain.certs().first().ok_or_else(|| unreadable("the private key has no certificate".into()))?;
        let key_der = Zeroizing::new(chain.key().as_der().to_vec());
        let key = InMemoryPrivateKey::from_pkcs8_der(&*key_der).map_err(key_error)?;
        Self::from_key(cert.as_der().to_vec(), key, team_id)
    }

    fn from_key(cert_der: Vec<u8>, key: InMemoryPrivateKey, team_id: Option<String>) -> Result<Self> {
        let cert = CapturedX509Certificate::from_der(cert_der.clone()).map_err(key_error)?;
        if cert.public_key_data() != key.public_key_data() {
            return Err(key_error("the private key does not match the certificate"));
        }
        let team_id = match team_id {
            Some(t) => t,
            None => certificate_info(&cert_der)?
                .team_id
                .ok_or_else(|| key_error("the certificate has no team (subject OU)"))?,
        };
        Ok(Self { cert_der, team_id, key })
    }

    /// The private key as PKCS#8 DER, for sealing in the secret store.
    pub fn private_key_pkcs8(&self) -> Result<Zeroizing<Vec<u8>>> {
        let pair = x509_certificate::InMemorySigningKeyPair::try_from(self.key.clone()).map_err(key_error)?;
        Ok(pair.to_pkcs8_one_asymmetric_key_der())
    }

    /// The certificate's facts.
    pub fn info(&self) -> Result<CertificateInfo> {
        certificate_info(&self.cert_der)
    }
}

impl CodeSigningIdentity for InMemoryIdentity {
    fn certificate_der(&self) -> &[u8] {
        &self.cert_der
    }
    fn team_id(&self) -> &str {
        &self.team_id
    }
    fn signer(&self) -> &dyn KeyInfoSigner {
        &self.key
    }
}

/// Decodes the first PEM block: `(label, DER)`.
pub(crate) fn decode_pem(text: &str) -> Option<(String, Zeroizing<Vec<u8>>)> {
    let begin = text.find("-----BEGIN ")?;
    let rest = &text[begin + 11..];
    let label_end = rest.find("-----")?;
    let label = rest[..label_end].to_owned();
    let body_start = label_end + 5;
    let end_marker = format!("-----END {label}-----");
    let body_end = rest.find(&end_marker)?;
    let b64: Zeroizing<String> =
        Zeroizing::new(rest[body_start..body_end].chars().filter(|c| !c.is_whitespace()).collect());
    let der = base64_decode(&b64)?;
    Some((label, Zeroizing::new(der)))
}

/// Standard base64 (with padding), without a dependency.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let bytes = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= val(c)? << (18 - 6 * i);
        }
        let n = match chunk.len() {
            4 => 3,
            3 => 2,
            2 => 1,
            _ => return None,
        };
        for i in 0..n {
            out.push((acc >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn base64_and_pem() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8hIQ==").unwrap(), b"hello!!");
        assert_eq!(base64_decode("YWJj").unwrap(), b"abc");
        assert!(base64_decode("a$==").is_none());
        let pem = "junk\n-----BEGIN TEST-----\naGVs\nbG8=\n-----END TEST-----\n";
        let (label, der) = decode_pem(pem).unwrap();
        assert_eq!(label, "TEST");
        assert_eq!(&*der, b"hello");
        assert!(decode_pem("-----BEGIN X-----\nAAAA\n").is_none());
    }
}
