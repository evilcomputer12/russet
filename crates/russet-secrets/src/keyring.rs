// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The master key in the OS keyring through keyring-core (core §9.6) [D 13]: the Secret Service on Linux
//! (`zbus-secret-service-keyring-store`, pure Rust, no libdbus) and the Credential Manager on Windows.
//!
//! The entry is service `dev.russet`, user `master-key`, or `master-key-<first 8 hex digits of
//! sha256(RUSSET_HOME)>` when `RUSSET_HOME` is set, so development worktrees and tests never share or reset one
//! key (ARCHITECTURE §3.3 item 7) [D 83]. The 32-byte key is stored base64-encoded as the entry's password, which
//! sidesteps binary-secret quirks of some Secret Service implementations [SIGN V16].
//!
//! keyring-core is synchronous and the zbus store blocks on its own runtime, so every call here runs on a
//! blocking thread (the async [`crate::SecretStore`] methods use `spawn_blocking`), never on a runtime worker.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use russet_core::error::{Error, Result, codes};
use zeroize::Zeroizing;

use crate::crypto::{self, MasterKey};
use crate::sealed::MasterKeySource;
use crate::{BackendInfo, BackendKind};

/// The keyring service of every Russet entry.
pub const SERVICE: &str = "dev.russet";

/// The master-key entry's user name for a `RUSSET_HOME` value (`None` when it is not set).
pub fn entry_name(russet_home: Option<&Path>) -> String {
    match russet_home {
        Some(home) => {
            let hash = crypto::sha256_hex(home.as_os_str().as_encoded_bytes());
            format!("master-key-{}", &hash[..8])
        }
        None => "master-key".to_owned(),
    }
}

/// The master key in one keyring entry.
#[derive(Debug, Clone)]
pub struct KeyringKeySource {
    entry: String,
}

impl KeyringKeySource {
    /// The source for entry `entry` of service [`SERVICE`].
    pub fn new(entry: impl Into<String>) -> Self {
        Self { entry: entry.into() }
    }

    /// The entry's user name.
    pub fn entry(&self) -> &str {
        &self.entry
    }

    fn open_entry(&self) -> Result<keyring_core::Entry> {
        let store = platform_store()?;
        store.build(SERVICE, &self.entry, None).map_err(|e| map_keyring_error(e, &self.entry))
    }

    fn info(&self, available: bool, locked: bool, reason: Option<String>) -> BackendInfo {
        BackendInfo { kind: BackendKind::Keyring, available, locked, reason }
    }

    /// Deletes the entry (`russet secrets` never does this on its own; tests clean up their own entries).
    pub fn delete_entry(&self) -> Result<bool> {
        match self.open_entry()?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(e) => Err(map_keyring_error(e, &self.entry)),
        }
    }
}

#[cfg(target_os = "linux")]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>> {
    let store =
        zbus_secret_service_keyring_store::Store::new().map_err(|e| map_keyring_error(e, "the Secret Service"))?;
    Ok(store)
}

#[cfg(windows)]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>> {
    let store =
        windows_native_keyring_store::Store::new().map_err(|e| map_keyring_error(e, "the Credential Manager"))?;
    Ok(store)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>> {
    Err(Error::new(codes::secrets::UNAVAILABLE, "no OS keyring is supported on this platform").with_hint(
        "Set RUSSET_SECRETS_BACKEND=passphrase and RUSSET_SECRETS_PASSPHRASE, or env for CI (ARCHITECTURE §14.9).",
    ))
}

fn unavailable_hint() -> &'static str {
    if cfg!(windows) {
        "The Windows Credential Manager is not reachable; set RUSSET_SECRETS_BACKEND=passphrase and \
         RUSSET_SECRETS_PASSPHRASE to use the passphrase store instead."
    } else {
        "Start a Secret Service (KDE Wallet/ksecretd or GNOME Keyring) in this session, or set \
         RUSSET_SECRETS_BACKEND=passphrase and RUSSET_SECRETS_PASSPHRASE for a headless host."
    }
}

/// Maps keyring-core errors to the `secrets` family.
fn map_keyring_error(e: keyring_core::Error, what: &str) -> Error {
    match e {
        keyring_core::Error::NoStorageAccess(_) => {
            Error::new(codes::secrets::LOCKED, format!("the keyring is locked or refused access to {what}: {e}"))
                .with_hint("Unlock your keyring (KDE Wallet or GNOME Keyring) and retry.")
                .retryable(true)
        }
        keyring_core::Error::NoDefaultStore | keyring_core::Error::PlatformFailure(_) => {
            Error::new(codes::secrets::UNAVAILABLE, format!("the OS keyring is not available ({what}): {e}"))
                .with_hint(unavailable_hint())
        }
        other => Error::new(codes::secrets::UNAVAILABLE, format!("the OS keyring failed ({what}): {other}"))
            .with_hint(unavailable_hint()),
    }
}

impl MasterKeySource for KeyringKeySource {
    fn kind(&self) -> BackendKind {
        BackendKind::Keyring
    }

    fn keyring_entry(&self) -> Option<String> {
        Some(self.entry.clone())
    }

    fn load(&self) -> Result<Option<MasterKey>> {
        match self.open_entry()?.get_password() {
            Ok(text) => {
                let text = Zeroizing::new(text);
                let bytes = Zeroizing::new(STANDARD.decode(text.trim()).map_err(|_| {
                    Error::new(
                        codes::secrets::LOCKED,
                        format!("the keyring entry {} is not a Russet master key", self.entry),
                    )
                    .with_hint(
                        "The entry was changed outside Russet; restore it, or remove it and the secrets directory.",
                    )
                })?);
                MasterKey::from_bytes(&bytes).map(Some)
            }
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_keyring_error(e, &self.entry)),
        }
    }

    fn create(&self) -> Result<MasterKey> {
        let key = MasterKey::generate()?;
        let encoded = Zeroizing::new(STANDARD.encode(key.expose()));
        let entry = self.open_entry()?;
        entry.set_password(&encoded).map_err(|e| map_keyring_error(e, &self.entry))?;
        // Read it back: some stores accept a write they cannot return.
        match self.load()? {
            Some(k) if k.expose() == key.expose() => Ok(key),
            _ => Err(Error::new(
                codes::secrets::UNAVAILABLE,
                format!("the keyring did not keep the entry {}", self.entry),
            )
            .with_hint(unavailable_hint())),
        }
    }

    fn probe(&self) -> BackendInfo {
        let entry = match self.open_entry() {
            Ok(e) => e,
            Err(e) => {
                let locked = e.code() == codes::secrets::LOCKED;
                return self.info(locked, locked, Some(e.message().to_owned()));
            }
        };
        match entry.get_password() {
            Ok(_) | Err(keyring_core::Error::NoEntry) => self.info(true, false, None),
            Err(e) => {
                let mapped = map_keyring_error(e, &self.entry);
                let locked = mapped.code() == codes::secrets::LOCKED;
                self.info(locked, locked, Some(mapped.message().to_owned()))
            }
        }
    }

    fn describe(&self) -> BackendInfo {
        self.info(true, false, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_names_are_per_home() {
        assert_eq!(entry_name(None), "master-key");
        let a = entry_name(Some(Path::new("/home/u/russet-w3/.russet-dev")));
        let b = entry_name(Some(Path::new("/home/u/russet-w4/.russet-dev")));
        assert!(a.starts_with("master-key-") && a.len() == "master-key-".len() + 8, "{a}");
        assert_ne!(a, b, "two homes get two entries");
        assert_eq!(a, entry_name(Some(Path::new("/home/u/russet-w3/.russet-dev"))), "stable");
        // printf %s "$RUSSET_HOME" | sha256sum | cut -c1-8
        let expected = &crypto::sha256_hex(b"/home/u/russet-w3/.russet-dev")[..8];
        assert_eq!(a, format!("master-key-{expected}"));
    }
}
