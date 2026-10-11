// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! App Store Connect keys and ES256 JWTs (SIGN §5.2): header `{alg: ES256, kid, typ: JWT}`, claims
//! `{iss | sub: "user", iat, exp, aud: "appstoreconnect-v1", scope?}`, signed with the key's P-256 private half.

use std::time::{Duration, SystemTime};

use p256::ecdsa::SigningKey;
use p256::ecdsa::signature::Signer as _;
use p256::pkcs8::DecodePrivateKey as _;
use russet_core::Result;
use russet_core::error::{Error, codes};
use secrecy::SecretString;
use zeroize::Zeroizing;

/// Who issued a key: a team key (with its issuer ID) or an individual key (`sub: "user"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Issuer {
    Team { issuer_id: String },
    Individual,
}

/// An App Store Connect API key: ID, issuer and the parsed P-256 private key. The key never leaves memory and
/// never appears in `Debug` output.
#[derive(Clone)]
pub struct AscKey {
    pub key_id: String,
    pub issuer: Issuer,
    signing: SigningKey,
}

impl std::fmt::Debug for AscKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AscKey").field("key_id", &self.key_id).field("issuer", &self.issuer).finish_non_exhaustive()
    }
}

/// `account.keyInvalid` with a hint.
fn key_invalid(why: impl std::fmt::Display) -> Error {
    Error::new(codes::account::KEY_INVALID, format!("the App Store Connect key is unusable: {why}"))
        .with_hint("Use the AuthKey_<KEYID>.p8 file App Store Connect gave you (an EC P-256 PKCS#8 PEM key).")
}

impl AscKey {
    /// Parses a `.p8` (PKCS#8 PEM, EC P-256). Validates key ID and issuer formats loosely: Apple's key IDs are
    /// 10 characters, issuer IDs UUIDs.
    pub fn from_p8(key_id: &str, issuer: Issuer, pem: &[u8]) -> Result<Self> {
        let pem = std::str::from_utf8(pem).map_err(|_| key_invalid("the file is not text"))?;
        let pem = Zeroizing::new(pem.trim().to_owned());
        let signing =
            SigningKey::from_pkcs8_pem(&pem).map_err(|_| key_invalid("not an EC P-256 PKCS#8 private key"))?;
        if key_id.is_empty() || !key_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, format!("invalid key ID `{key_id}`"))
                .with_hint("The key ID is the 10-character ID shown next to the key in App Store Connect."));
        }
        if let Issuer::Team { issuer_id } = &issuer
            && (issuer_id.is_empty() || !issuer_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "invalid issuer ID")
                .with_hint("The issuer ID is the UUID shown on the Integrations tab of App Store Connect."));
        }
        Ok(Self { key_id: key_id.to_owned(), issuer, signing })
    }

    /// Whether this is an individual key (no provisioning endpoints).
    pub fn is_individual(&self) -> bool {
        self.issuer == Issuer::Individual
    }

    /// The public key, as an uncompressed SEC1 point (for tests and fingerprints; not secret).
    pub fn public_key_sec1(&self) -> Vec<u8> {
        self.signing.verifying_key().to_sec1_point(false).as_bytes().to_vec()
    }

    /// Mints a JWT issued at `now`, valid for `lifetime`, optionally scoped.
    pub fn mint(&self, now: SystemTime, lifetime: Duration, scope: Option<&[String]>) -> Result<SecretString> {
        let iat = now.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let exp = iat + lifetime.as_secs();
        let header = serde_json::json!({"alg": "ES256", "kid": self.key_id, "typ": "JWT"});
        let mut claims = serde_json::Map::new();
        match &self.issuer {
            Issuer::Team { issuer_id } => {
                claims.insert("iss".into(), issuer_id.clone().into());
            }
            Issuer::Individual => {
                claims.insert("sub".into(), "user".into());
            }
        }
        claims.insert("iat".into(), iat.into());
        claims.insert("exp".into(), exp.into());
        claims.insert("aud".into(), "appstoreconnect-v1".into());
        if let Some(scope) = scope {
            claims.insert("scope".into(), scope.to_vec().into());
        }
        let encode = |v: &serde_json::Value| -> Result<String> {
            serde_json::to_vec(v)
                .map(|b| b64url(&b))
                .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode a JWT: {e}")))
        };
        let signing_input = format!("{}.{}", encode(&header)?, encode(&serde_json::Value::Object(claims))?);
        let sig: p256::ecdsa::Signature = self.signing.sign(signing_input.as_bytes());
        Ok(SecretString::from(format!("{signing_input}.{}", b64url(&sig.to_bytes()))))
    }
}

/// Base64url without padding (RFC 7515).
pub(crate) fn b64url(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(T[n as usize & 63] as char);
        }
    }
    out
}

/// Decodes base64url without padding; `None` on invalid input.
pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        })
    };
    let bytes = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        let take = match chunk.len() {
            4 => 3,
            3 => 2,
            2 => 1,
            _ => return None,
        };
        for i in 0..take {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

/// Reads the claims of a JWT without checking its signature (tests, fakes and diagnostics only).
pub fn unverified_claims(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    serde_json::from_slice(&b64url_decode(payload)?).ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn base64url_round_trips() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0xfb, 0xff, 0xfe]] {
            assert_eq!(b64url_decode(&b64url(data)).unwrap(), data);
        }
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
        assert!(b64url_decode("a").is_none());
        assert!(b64url_decode("a$cd").is_none());
    }
}
