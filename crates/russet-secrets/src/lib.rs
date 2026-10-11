// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The secret store (testability contract T6; ARCHITECTURE §5.3 `russet-secrets`, §12.2) [D 13]: a keyring-core
//! master key plus sealed files, a passphrase file store, an env store (CI) and a memory store (tests).
//! With `RUSSET_HOME` set, the keyring entry is `master-key-<sha256(RUSSET_HOME)[..8]>` [D 83].
//!
//! - [`SealedStore`](sealed::SealedStore) with [`KeyringKeySource`](keyring::KeyringKeySource) or
//!   [`PassphraseKeySource`](passphrase::PassphraseKeySource): `data/secrets/<sha256(id)>.sealed`
//!   (XChaCha20-Poly1305, AAD = id ‖ kind ‖ version), `index.json` (names only), `master.salt`.
//! - [`EnvStore`](env::EnvStore): read-only `RUSSET_ASC_*` and `RUSSET_SIGNING_P12_*` (CI).
//! - [`MemoryStore`](memory::MemoryStore): tests.
//! - [`redact`]: field-name and value redaction for logs and reports.
//!
//! Values never cross IPC: `secrets.*` methods return [`SecretMeta`] only. [`SecretBytes`] zeroizes on drop and
//! has a redacting `Debug`, no `Display` and no `Serialize`.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, codes};
use russet_core::secret::Redacted;
use time::OffsetDateTime;
use zeroize::Zeroizing;

pub mod crypto;
pub mod env;
pub mod keyring;
pub mod memory;
pub mod passphrase;
pub mod redact;
pub mod sealed;

/// The largest secret Russet stores (keys, tokens and `.p12` files are a few KiB).
pub const MAX_SECRET_BYTES: usize = 1 << 20;

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
    /// Every kind.
    pub const ALL: [SecretKind; 8] = [
        Self::AscKey,
        Self::SigningKey,
        Self::AppleIdSession,
        Self::AppleIdPassword,
        Self::AnisetteState,
        Self::GithubToken,
        Self::McpHttpToken,
        Self::TcpToken,
    ];

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

    /// Parses the string form (`asc-key`, …).
    pub fn parse(s: &str) -> Result<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s).ok_or_else(|| {
            let kinds: Vec<&str> = Self::ALL.iter().map(|k| k.as_str()).collect();
            Error::new(codes::core::INVALID_ARGUMENT, format!("`{s}` is not a secret kind"))
                .with_hint(format!("Use one of: {}.", kinds.join(", ")))
        })
    }
}

/// A secret's id; string form `<kind>/<name>`, for example `asc-key/2X9R4HXF34`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SecretId {
    pub kind: SecretKind,
    pub name: String,
}

impl SecretId {
    /// The prefix of a reference to a secret in a configuration file (`secret = "secret:asc-key/<KEYID>"`).
    pub const REFERENCE_PREFIX: &'static str = "secret:";

    /// Parses `<kind>/<name>`. Names are 1–256 characters without control characters; they may contain `/`.
    pub fn parse(s: &str) -> Result<Self> {
        let (kind, name) = s.split_once('/').ok_or_else(|| {
            Error::new(codes::core::INVALID_ARGUMENT, format!("`{s}` is not a secret id"))
                .with_hint("Write `<kind>/<name>`, for example `asc-key/2X9R4HXF34`.")
        })?;
        let kind = SecretKind::parse(kind)?;
        if name.is_empty() || name.chars().count() > 256 || name.chars().any(char::is_control) {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, format!("`{s}` has an invalid name"))
                .with_hint("Names are 1–256 characters without control characters."));
        }
        Ok(Self { kind, name: name.to_owned() })
    }

    /// Parses a configuration reference, `secret:<kind>/<name>`.
    pub fn from_reference(r: &str) -> Result<Self> {
        let rest = r.strip_prefix(Self::REFERENCE_PREFIX).ok_or_else(|| {
            Error::new(codes::config::INVALID, format!("`{r}` is not a secret reference"))
                .with_hint("Write `secret:<kind>/<name>`.")
        })?;
        Self::parse(rest)
    }

    /// The configuration reference, `secret:<kind>/<name>`.
    pub fn reference(&self) -> String {
        format!("{}{self}", Self::REFERENCE_PREFIX)
    }
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

impl BackendKind {
    /// The wire name: `keyring`, `passphrase`, `env`, `memory`.
    pub fn as_str(self) -> &'static str {
        sealed::backend_name(self)
    }
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
    /// The keyring entry holding the master key (`master-key` or `master-key-<hash8>`), for the keyring backend.
    /// Removing a worktree also removes this entry (PLAN §14).
    pub keyring_entry: Option<String>,
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

impl BackendChoice {
    /// Parses `RUSSET_SECRETS_BACKEND`.
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim() {
            "" | "auto" => Ok(Self::Auto),
            "keyring" => Ok(Self::Keyring),
            "passphrase" => Ok(Self::Passphrase),
            "env" => Ok(Self::Env),
            "memory" => Ok(Self::Memory),
            other => Err(Error::new(
                codes::secrets::UNAVAILABLE,
                format!("RUSSET_SECRETS_BACKEND=`{other}` is not a secret backend"),
            )
            .with_hint("Use auto, keyring, passphrase, env, or memory (tests).")),
        }
    }
}

/// Secret-store configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretsConfig {
    pub backend: BackendChoice,
    /// `RUSSET_HOME`, when set: the keyring entry becomes `master-key-<sha256(RUSSET_HOME)[..8]>` [D 83].
    pub russet_home: Option<PathBuf>,
    /// An explicit keyring entry (the env-gated keyring test uses a unique one); overrides the default name.
    pub keyring_entry: Option<String>,
    /// The passphrase store's passphrase (`RUSSET_SECRETS_PASSPHRASE`).
    pub passphrase: Option<Redacted<String>>,
    /// Whether a D-Bus session bus is configured (Linux): `auto` prefers the keyring only then.
    pub session_bus: bool,
}

impl SecretsConfig {
    /// Reads `RUSSET_SECRETS_BACKEND`, `RUSSET_SECRETS_PASSPHRASE`, `RUSSET_HOME` and the D-Bus session address.
    pub fn from_env() -> Result<Self> {
        let var = |v: &str| std::env::var(v).ok().filter(|s| !s.is_empty());
        Ok(Self {
            backend: var("RUSSET_SECRETS_BACKEND").map(|s| BackendChoice::parse(&s)).transpose()?.unwrap_or_default(),
            russet_home: var("RUSSET_HOME").map(PathBuf::from),
            keyring_entry: None,
            passphrase: var(passphrase::PASSPHRASE_VAR).map(Redacted),
            session_bus: cfg!(windows)
                || var("DBUS_SESSION_BUS_ADDRESS").is_some()
                || var("XDG_RUNTIME_DIR").is_some_and(|d| std::path::Path::new(&d).join("bus").exists()),
        })
    }

    /// The keyring entry name this configuration uses.
    pub fn keyring_entry_name(&self) -> String {
        self.keyring_entry.clone().unwrap_or_else(|| keyring::entry_name(self.russet_home.as_deref()))
    }
}

/// A secret store.
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn put(&self, id: &SecretId, label: &str, v: SecretBytes) -> Result<SecretMeta>;
    /// In-process only; never over IPC.
    async fn get(&self, id: &SecretId) -> Result<Option<SecretBytes>>;
    async fn delete(&self, id: &SecretId) -> Result<bool>;
    async fn list(&self, kind: Option<SecretKind>) -> Result<Vec<SecretMeta>>;
    /// The backend and its last known state, without I/O.
    fn backend(&self) -> BackendInfo;
    /// The backend's current state: probes the keyring or checks the passphrase, never creates anything
    /// (`secrets.status`, doctor's `secrets.backend`).
    async fn status(&self) -> BackendInfo {
        self.backend()
    }
}

/// `store.get(id)`, with `secrets.notFound` (1402) when the secret does not exist.
pub async fn require(store: &dyn SecretStore, id: &SecretId) -> Result<SecretBytes> {
    store.get(id).await?.ok_or_else(|| {
        Error::new(codes::secrets::NOT_FOUND, format!("no secret {id}"))
            .with_hint("Store it again: `russet account import --from-xtool` or `russet secrets set`.")
            .with_details(serde_json::json!({ "id": id.to_string() }))
    })
}

pub(crate) fn check_size(v: &SecretBytes) -> Result<()> {
    if v.len() > MAX_SECRET_BYTES {
        return Err(Error::new(
            codes::core::INVALID_ARGUMENT,
            format!("the secret is {} bytes; the limit is {MAX_SECRET_BYTES}", v.len()),
        )
        .with_hint("Secrets are keys and tokens; store large files elsewhere."));
    }
    Ok(())
}

/// The directory of the sealed files: `<data>/secrets`.
pub fn secrets_dir(dirs: &RussetDirs) -> PathBuf {
    dirs.data.join("secrets")
}

/// Opens the store `cfg` selects. Nothing here blocks or touches the keyring: the keyring is reached on first use
/// (on a blocking thread), so a daemon starts even without one and reports the state through `secrets.status`.
///
/// `auto` keeps using the backend that sealed `data/secrets` (from `store.json`); for a new store it takes the
/// keyring when a session bus is configured, else the passphrase store when `RUSSET_SECRETS_PASSPHRASE` is set,
/// else the keyring (which then reports itself unavailable, with a hint).
pub fn open_default(dirs: &RussetDirs, cfg: &SecretsConfig) -> Result<Arc<dyn SecretStore>> {
    let dir = secrets_dir(dirs);
    let choice = match cfg.backend {
        BackendChoice::Auto => match sealed::sealed_by(&dir) {
            Some(BackendKind::PassphraseFile) => BackendChoice::Passphrase,
            Some(_) => BackendChoice::Keyring,
            None if cfg.session_bus => BackendChoice::Keyring,
            None if cfg.passphrase.is_some() => BackendChoice::Passphrase,
            None => BackendChoice::Keyring,
        },
        explicit => explicit,
    };
    if let Some(sealed_with) = sealed::sealed_by(&dir) {
        let wanted = match choice {
            BackendChoice::Keyring => Some(BackendKind::Keyring),
            BackendChoice::Passphrase => Some(BackendKind::PassphraseFile),
            _ => None,
        };
        if let Some(w) = wanted
            && w != sealed_with
        {
            return Err(Error::new(
                codes::secrets::LOCKED,
                format!("{} was sealed by the {} backend, not {}", dir.display(), sealed_with.as_str(), w.as_str()),
            )
            .with_hint("Unset RUSSET_SECRETS_BACKEND (auto keeps the sealing backend), or move the directory away."));
        }
    }
    Ok(match choice {
        BackendChoice::Memory => Arc::new(memory::MemoryStore::new()),
        BackendChoice::Env => Arc::new(env::EnvStore::from_env()),
        BackendChoice::Passphrase => Arc::new(sealed::SealedStore::new(
            dir.clone(),
            Box::new(passphrase::PassphraseKeySource::new(
                dir,
                cfg.passphrase.as_ref().map(|p| p.0.clone()),
                passphrase::KdfCost::DEFAULT,
            )),
        )),
        BackendChoice::Keyring | BackendChoice::Auto => {
            Arc::new(sealed::SealedStore::new(dir, Box::new(keyring::KeyringKeySource::new(cfg.keyring_entry_name()))))
        }
    })
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

    #[test]
    fn ids_parse_and_reference() {
        let id = SecretId::parse("signing-key/TEAM/0A1B").expect("id with a slash in the name");
        assert_eq!((id.kind, id.name.as_str()), (SecretKind::SigningKey, "TEAM/0A1B"));
        assert_eq!(id.reference(), "secret:signing-key/TEAM/0A1B");
        assert_eq!(SecretId::from_reference("secret:asc-key/K").map(|i| i.to_string()).ok(), Some("asc-key/K".into()));
        for bad in ["asc-key", "nope/x", "asc-key/", "asc-key/a\nb"] {
            assert!(SecretId::parse(bad).is_err(), "{bad}");
        }
        for k in SecretKind::ALL {
            assert_eq!(SecretKind::parse(k.as_str()).ok(), Some(k));
        }
    }

    #[test]
    fn backend_choices_parse() {
        assert_eq!(BackendChoice::parse("").ok(), Some(BackendChoice::Auto));
        assert_eq!(BackendChoice::parse("passphrase").ok(), Some(BackendChoice::Passphrase));
        assert!(BackendChoice::parse("vault").is_err());
    }
}
