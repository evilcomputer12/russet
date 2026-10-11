// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The secret store (testability contract T6; ARCHITECTURE §5.3 `russet-secrets`) [D 13]: a keyring-core
//! master key plus sealed files, a passphrase file store, an env store (CI) and a memory store (tests).
//! With `RUSSET_HOME` set, the keyring entry is `master-key-<sha256(RUSSET_HOME)[..8]>` [D 83].
//!
//! F0 stub (W1); W3 fills the bodies (CORE-09 = SA-01).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use russet_core::Result;
use russet_core::dirs::RussetDirs;
use time::OffsetDateTime;
use zeroize::Zeroizing;

/// What a secret is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SecretKind {
    AscKey,
    SigningKey,
    AppleIdSession,
    AppleIdPassword,
    AnisetteState,
    GithubToken,
    McpHttpToken,
    TcpToken,
}

impl SecretKind {
    /// The kind as it appears in a [`SecretId`]'s string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AscKey => "asc-key",
            Self::SigningKey => "signing-key",
            Self::AppleIdSession => "apple-id-session",
            Self::AppleIdPassword => "apple-id-password",
            Self::AnisetteState => "anisette-state",
            Self::GithubToken => "github-token",
            Self::McpHttpToken => "mcp-http-token",
            Self::TcpToken => "tcp-token",
        }
    }
}

/// A secret's id; string form `<kind>/<name>`, for example `asc-key/2X9R4HXF34`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SecretId {
    pub kind: SecretKind,
    pub name: String,
}

impl fmt::Display for SecretId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.kind.as_str(), self.name)
    }
}

/// Secret bytes: zeroized on drop, a redacting `Debug`, and no `Display` or `Serialize`.
pub struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }
    /// The plaintext, in-process only; never over IPC.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretBytes(*** {} bytes)", self.0.len())
    }
}

/// Which backend holds a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    Keyring,
    PassphraseFile,
    Env,
    Memory,
}

/// Metadata of one secret (the only thing `secrets.list` returns).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMeta {
    pub id: SecretId,
    pub label: String,
    pub backend: BackendKind,
    pub created_at: OffsetDateTime,
    pub size_bytes: usize,
}

/// The store's backend and state (`secrets.status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendInfo {
    pub kind: BackendKind,
    pub available: bool,
    pub locked: bool,
    pub reason: Option<String>,
}

/// The backend choice (`RUSSET_SECRETS_BACKEND`: `auto|keyring|passphrase|env`, plus `memory` for tests).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BackendChoice {
    #[default]
    Auto,
    Keyring,
    Passphrase,
    Env,
    Memory,
}

/// Secret-store configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretsConfig {
    pub backend: BackendChoice,
}

/// A secret store.
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn put(&self, id: &SecretId, label: &str, v: SecretBytes) -> Result<SecretMeta>;
    /// In-process only; never over IPC.
    async fn get(&self, id: &SecretId) -> Result<Option<SecretBytes>>;
    async fn delete(&self, id: &SecretId) -> Result<bool>;
    async fn list(&self, kind: Option<SecretKind>) -> Result<Vec<SecretMeta>>;
    fn backend(&self) -> BackendInfo;
}

/// Opens the store `cfg` selects.
pub fn open_default(dirs: &RussetDirs, cfg: &SecretsConfig) -> Result<Arc<dyn SecretStore>> {
    todo!("W3 (CORE-09)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_bytes_render_safely() {
        let id = SecretId { kind: SecretKind::AscKey, name: "2X9R4HXF34".into() };
        assert_eq!(id.to_string(), "asc-key/2X9R4HXF34");
        let b = SecretBytes::new(b"canary-value".to_vec());
        assert_eq!(format!("{b:?}"), "SecretBytes(*** 12 bytes)");
    }
}
