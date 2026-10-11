// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The CI store (`RUSSET_SECRETS_BACKEND=env`, ARCHITECTURE §14.9): read-only secrets from environment variables,
//! kept in memory and never written anywhere [D 13].
//!
//! | Variables | Secret id |
//! |---|---|
//! | `RUSSET_ASC_KEY_ID` with `RUSSET_ASC_KEY_P8` (PEM, or base64 of it) or `RUSSET_ASC_KEY_PATH` | `asc-key/<RUSSET_ASC_KEY_ID>` |
//! | `RUSSET_SIGNING_P12_B64` | `signing-key/ci` (the PKCS#12 bytes) |
//! | `RUSSET_SIGNING_P12_PASSWORD` | `signing-key/ci.password` |
//!
//! `RUSSET_ASC_ISSUER_ID` is not a secret; the account service reads it directly.

use std::collections::BTreeMap;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use russet_core::error::{Error, Result, codes};
use time::OffsetDateTime;

use crate::{BackendInfo, BackendKind, SecretBytes, SecretId, SecretKind, SecretMeta, SecretStore};

/// The id of the CI signing identity's PKCS#12.
pub const CI_P12: &str = "ci";
/// The id of its password.
pub const CI_P12_PASSWORD: &str = "ci.password";

/// Read-only secrets from the environment.
pub struct EnvStore {
    items: BTreeMap<SecretId, (SecretMeta, SecretBytes)>,
    problems: Vec<String>,
}

impl std::fmt::Debug for EnvStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvStore")
            .field("ids", &self.items.keys().map(ToString::to_string).collect::<Vec<_>>())
            .finish()
    }
}

impl EnvStore {
    /// Reads the variables of this process.
    pub fn from_env() -> Self {
        Self::from_lookup(|v| std::env::var(v).ok())
    }

    /// Reads the variables through `lookup` (tests pass a map).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut store = Self { items: BTreeMap::new(), problems: Vec::new() };
        let get = |v: &str| lookup(v).filter(|s| !s.trim().is_empty());
        if let Some(key_id) = get("RUSSET_ASC_KEY_ID") {
            let p8 = match (get("RUSSET_ASC_KEY_P8"), get("RUSSET_ASC_KEY_PATH")) {
                (Some(text), _) => decode_pem_or_base64(&text),
                (None, Some(path)) => std::fs::read(&path).map_err(|e| format!("cannot read RUSSET_ASC_KEY_PATH: {e}")),
                (None, None) => Err("RUSSET_ASC_KEY_ID is set without RUSSET_ASC_KEY_P8 or RUSSET_ASC_KEY_PATH".into()),
            };
            match p8 {
                Ok(bytes) => store.insert(SecretKind::AscKey, key_id.trim(), "App Store Connect key (env)", bytes),
                Err(p) => store.problems.push(p),
            }
        }
        if let Some(b64) = get("RUSSET_SIGNING_P12_B64") {
            match STANDARD.decode(b64.trim()) {
                Ok(bytes) => store.insert(SecretKind::SigningKey, CI_P12, "CI signing identity (env)", bytes),
                Err(_) => store.problems.push("RUSSET_SIGNING_P12_B64 is not base64".into()),
            }
        }
        if let Some(pw) = get("RUSSET_SIGNING_P12_PASSWORD") {
            store.insert(
                SecretKind::SigningKey,
                CI_P12_PASSWORD,
                "CI signing identity password (env)",
                pw.into_bytes(),
            );
        }
        store
    }

    fn insert(&mut self, kind: SecretKind, name: &str, label: &str, bytes: Vec<u8>) {
        let id = SecretId { kind, name: name.to_owned() };
        let meta = SecretMeta {
            id: id.clone(),
            label: label.to_owned(),
            backend: BackendKind::Env,
            created_at: OffsetDateTime::now_utc(),
            size_bytes: bytes.len(),
        };
        self.items.insert(id, (meta, SecretBytes::new(bytes)));
    }

    fn read_only(&self) -> Error {
        Error::new(codes::secrets::UNAVAILABLE, "the env secret backend is read-only").with_hint(
            "In CI, pass secrets through RUSSET_ASC_* and RUSSET_SIGNING_P12_*; elsewhere use the keyring or \
             passphrase backend (RUSSET_SECRETS_BACKEND).",
        )
    }
}

fn decode_pem_or_base64(text: &str) -> std::result::Result<Vec<u8>, String> {
    let t = text.trim();
    if t.starts_with("-----BEGIN") {
        return Ok(format!("{t}\n").into_bytes());
    }
    STANDARD.decode(t).map_err(|_| "RUSSET_ASC_KEY_P8 is neither PEM nor base64".to_owned())
}

#[async_trait]
impl SecretStore for EnvStore {
    async fn put(&self, _id: &SecretId, _label: &str, _v: SecretBytes) -> Result<SecretMeta> {
        Err(self.read_only())
    }

    async fn get(&self, id: &SecretId) -> Result<Option<SecretBytes>> {
        Ok(self.items.get(id).map(|(_, v)| SecretBytes::new(v.expose().to_vec())))
    }

    async fn delete(&self, _id: &SecretId) -> Result<bool> {
        Err(self.read_only())
    }

    async fn list(&self, kind: Option<SecretKind>) -> Result<Vec<SecretMeta>> {
        Ok(self.items.values().map(|(m, _)| m.clone()).filter(|m| kind.is_none_or(|k| m.id.kind == k)).collect())
    }

    fn backend(&self) -> BackendInfo {
        BackendInfo {
            kind: BackendKind::Env,
            available: true,
            locked: false,
            reason: (!self.problems.is_empty()).then(|| self.problems.join("; ")),
            keyring_entry: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ci_variables_map_to_ids() {
        let pem = "-----BEGIN PRIVATE KEY-----\nMIGT\n-----END PRIVATE KEY-----";
        let env = BTreeMap::from([
            ("RUSSET_ASC_KEY_ID", "2X9R4HXF34".to_owned()),
            ("RUSSET_ASC_KEY_P8", STANDARD.encode(pem)),
            ("RUSSET_SIGNING_P12_B64", STANDARD.encode([1u8, 2, 3])),
            ("RUSSET_SIGNING_P12_PASSWORD", "pw".to_owned()),
        ]);
        let s = EnvStore::from_lookup(|v| env.get(v).cloned());
        let asc = SecretId { kind: SecretKind::AscKey, name: "2X9R4HXF34".into() };
        let got = s.get(&asc).await.expect("get").expect("asc key");
        assert_eq!(got.expose(), pem.as_bytes(), "base64 of the PEM decodes to the PEM");
        let p12 = SecretId { kind: SecretKind::SigningKey, name: CI_P12.into() };
        assert_eq!(s.get(&p12).await.expect("get").expect("p12").expose(), &[1, 2, 3]);
        assert_eq!(s.list(Some(SecretKind::SigningKey)).await.expect("list").len(), 2);
        assert_eq!(
            s.put(&asc, "x", SecretBytes::new(vec![1])).await.map_err(|e| e.code()).err(),
            Some(codes::secrets::UNAVAILABLE)
        );
        assert!(s.backend().reason.is_none());
        let partial = EnvStore::from_lookup(|v| (v == "RUSSET_ASC_KEY_ID").then(|| "K".to_owned()));
        assert!(partial.backend().reason.is_some_and(|r| r.contains("RUSSET_ASC_KEY_P8")));
    }
}
