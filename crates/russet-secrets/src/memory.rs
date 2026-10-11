// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The in-memory store for tests (`RUSSET_SECRETS_BACKEND=memory`): nothing touches the disk or a keyring.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use russet_core::error::Result;
use time::OffsetDateTime;

use crate::{BackendInfo, BackendKind, SecretBytes, SecretId, SecretKind, SecretMeta, SecretStore, check_size};

/// Secrets held in process memory (zeroized when replaced or dropped).
#[derive(Default)]
pub struct MemoryStore {
    items: Mutex<BTreeMap<SecretId, (SecretMeta, SecretBytes)>>,
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.items.lock().map(|m| m.len()).unwrap_or(0);
        f.debug_struct("MemoryStore").field("items", &n).finish()
    }
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn items(&self) -> std::sync::MutexGuard<'_, BTreeMap<SecretId, (SecretMeta, SecretBytes)>> {
        match self.items.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }
}

#[async_trait]
impl SecretStore for MemoryStore {
    async fn put(&self, id: &SecretId, label: &str, v: SecretBytes) -> Result<SecretMeta> {
        check_size(&v)?;
        let meta = SecretMeta {
            id: id.clone(),
            label: label.to_owned(),
            backend: BackendKind::Memory,
            created_at: OffsetDateTime::now_utc(),
            size_bytes: v.len(),
        };
        self.items().insert(id.clone(), (meta.clone(), v));
        Ok(meta)
    }

    async fn get(&self, id: &SecretId) -> Result<Option<SecretBytes>> {
        Ok(self.items().get(id).map(|(_, v)| SecretBytes::new(v.expose().to_vec())))
    }

    async fn delete(&self, id: &SecretId) -> Result<bool> {
        Ok(self.items().remove(id).is_some())
    }

    async fn list(&self, kind: Option<SecretKind>) -> Result<Vec<SecretMeta>> {
        Ok(self.items().values().map(|(m, _)| m.clone()).filter(|m| kind.is_none_or(|k| m.id.kind == k)).collect())
    }

    fn backend(&self) -> BackendInfo {
        BackendInfo { kind: BackendKind::Memory, available: true, locked: false, reason: None }
    }
}
