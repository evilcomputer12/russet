// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The headless master key (core §9.6): derived from a passphrase with Argon2id (m = 64 MiB, t = 3, p = 1) and a
//! random salt in `data/secrets/master.salt`. Used over SSH, in containers and wherever no Secret Service runs;
//! it never touches D-Bus. The passphrase comes from `RUSSET_SECRETS_PASSPHRASE` (or the caller).

use std::path::PathBuf;

use argon2::{Algorithm, Argon2, Params, Version};
use russet_core::error::{Error, Result, codes};
use russet_core::secret::Redacted;
use zeroize::Zeroizing;

use crate::crypto::{self, MasterKey};
use crate::sealed::{self, MasterKeySource};
use crate::{BackendInfo, BackendKind};

/// The salt file.
pub const SALT_FILE: &str = "master.salt";
/// The variable that holds the passphrase.
pub const PASSPHRASE_VAR: &str = "RUSSET_SECRETS_PASSPHRASE";

/// Argon2id cost: memory in KiB, iterations, lanes (core §9.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfCost {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfCost {
    /// m = 64 MiB, t = 3, p = 1.
    pub const DEFAULT: KdfCost = KdfCost { m_kib: 64 * 1024, t: 3, p: 1 };
}

/// A master key derived from a passphrase.
pub struct PassphraseKeySource {
    dir: PathBuf,
    passphrase: Option<Redacted<Zeroizing<String>>>,
    cost: KdfCost,
}

impl std::fmt::Debug for PassphraseKeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassphraseKeySource")
            .field("dir", &self.dir)
            .field("passphrase", &self.passphrase.as_ref().map(|_| "***"))
            .field("cost", &self.cost)
            .finish()
    }
}

impl PassphraseKeySource {
    /// A source for the store in `dir`; `passphrase` is `None` when none was supplied.
    pub fn new(dir: PathBuf, passphrase: Option<String>, cost: KdfCost) -> Self {
        Self { dir, passphrase: passphrase.filter(|p| !p.is_empty()).map(|p| Redacted(Zeroizing::new(p))), cost }
    }

    /// Whether a passphrase was supplied.
    pub fn has_passphrase(&self) -> bool {
        self.passphrase.is_some()
    }

    fn passphrase(&self) -> Result<&str> {
        self.passphrase.as_ref().map(|p| p.0.as_str()).ok_or_else(|| {
            Error::new(codes::secrets::PASSPHRASE_REQUIRED, "the passphrase store needs a passphrase").with_hint(
                "Set RUSSET_SECRETS_PASSPHRASE in the daemon's environment (restart it with `russet daemon restart`), \
                 or use the OS keyring (RUSSET_SECRETS_BACKEND=keyring).",
            )
        })
    }

    fn derive(&self, salt: &[u8]) -> Result<MasterKey> {
        let params = Params::new(self.cost.m_kib, self.cost.t, self.cost.p, Some(32)).map_err(|e| {
            Error::new(codes::core::INTERNAL, format!("invalid Argon2 parameters: {e}")).with_hint("Report this bug.")
        })?;
        let mut out = Zeroizing::new([0u8; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(self.passphrase()?.as_bytes(), salt, out.as_mut())
            .map_err(|e| {
                Error::new(codes::core::INTERNAL, format!("passphrase key derivation failed: {e}"))
                    .with_hint("Report this bug.")
            })?;
        MasterKey::from_bytes(out.as_ref())
    }

    fn salt_path(&self) -> PathBuf {
        self.dir.join(SALT_FILE)
    }

    fn info(&self, available: bool, locked: bool, reason: Option<String>) -> BackendInfo {
        BackendInfo { kind: BackendKind::PassphraseFile, available, locked, reason }
    }
}

impl MasterKeySource for PassphraseKeySource {
    fn kind(&self) -> BackendKind {
        BackendKind::PassphraseFile
    }

    fn load(&self) -> Result<Option<MasterKey>> {
        let salt = match std::fs::read(self.salt_path()) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(Error::new(codes::core::IO, format!("cannot read {}: {e}", self.salt_path().display()))
                    .with_hint("Check the permissions of Russet's data directory.")
                    .caused_by(e));
            }
        };
        self.derive(&salt).map(Some)
    }

    fn create(&self) -> Result<MasterKey> {
        self.passphrase()?;
        let mut salt = [0u8; 16];
        crypto::random_fill(&mut salt)?;
        sealed::ensure_private_dir(&self.dir)?;
        sealed::write_private(&self.salt_path(), &salt)?;
        self.derive(&salt)
    }

    fn probe(&self) -> BackendInfo {
        match self.passphrase() {
            Ok(_) => self.info(true, false, None),
            Err(e) => self.info(true, true, Some(e.message().to_owned())),
        }
    }

    fn describe(&self) -> BackendInfo {
        if self.has_passphrase() {
            self.info(true, false, None)
        } else {
            self.info(true, true, Some("RUSSET_SECRETS_PASSPHRASE is not set".into()))
        }
    }
}
