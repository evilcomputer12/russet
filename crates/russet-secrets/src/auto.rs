// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `auto` for a new store with both a session bus and a passphrase (core §9.6): the OS keyring when it answers,
//! else the passphrase store. The choice is made on first use (probing the keyring on a blocking thread, never
//! at open) and kept; once a store is sealed, `store.json` decides and this wrapper is not used.

use std::sync::Mutex;

use async_trait::async_trait;
use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::sealed::SealedStore;
use crate::{BackendInfo, SecretBytes, SecretId, SecretKind, SecretMeta, SecretStore};

/// Keyring first, passphrase when the keyring is unavailable or locked.
pub struct AutoStore {
    keyring: SealedStore,
    passphrase: SealedStore,
    chosen: Mutex<Option<bool>>,
}

impl std::fmt::Debug for AutoStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoStore").field("chosen_passphrase", &self.chosen.lock().ok().and_then(|g| *g)).finish()
    }
}

impl AutoStore {
    /// Both candidate stores over the same directory.
    pub fn new(keyring: SealedStore, passphrase: SealedStore) -> Self {
        Self { keyring, passphrase, chosen: Mutex::new(None) }
    }

    fn cached(&self) -> Option<bool> {
        match self.chosen.lock() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        }
    }

    /// The store to use, probing the keyring once.
    async fn store(&self) -> Result<&SealedStore> {
        let use_passphrase = match self.cached() {
            Some(c) => c,
            None => {
                let keyring = self.keyring.clone();
                let info = tokio::task::spawn_blocking(move || keyring.status_sync()).await.map_err(|e| {
                    Error::new(codes::core::INTERNAL, format!("the keyring probe failed: {e}"))
                        .with_hint("Report this bug.")
                })?;
                let c = !(info.available && !info.locked);
                if let Ok(mut g) = self.chosen.lock() {
                    *g = Some(c);
                }
                c
            }
        };
        Ok(if use_passphrase { &self.passphrase } else { &self.keyring })
    }
}

#[async_trait]
impl SecretStore for AutoStore {
    async fn put(&self, id: &SecretId, label: &str, v: SecretBytes) -> Result<SecretMeta> {
        self.store().await?.put(id, label, v).await
    }

    async fn get(&self, id: &SecretId) -> Result<Option<SecretBytes>> {
        self.store().await?.get(id).await
    }

    async fn delete(&self, id: &SecretId) -> Result<bool> {
        self.store().await?.delete(id).await
    }

    async fn list(&self, kind: Option<SecretKind>) -> Result<Vec<SecretMeta>> {
        self.store().await?.list(kind).await
    }

    fn backend(&self) -> BackendInfo {
        match self.cached() {
            Some(true) => self.passphrase.backend(),
            Some(false) => self.keyring.backend(),
            None => BackendInfo {
                reason: Some("not probed yet; the passphrase store is used if the keyring does not answer".into()),
                ..self.keyring.backend()
            },
        }
    }

    async fn status(&self) -> BackendInfo {
        match self.store().await {
            Ok(s) => s.status().await,
            Err(e) => BackendInfo { reason: Some(e.message().to_owned()), ..self.keyring.backend() },
        }
    }
}
