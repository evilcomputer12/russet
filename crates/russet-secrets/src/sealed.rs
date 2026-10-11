// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The sealed-file store behind the keyring and passphrase backends (ARCHITECTURE §14.8, §12.2) [D 13]:
//!
//! - `data/secrets/<sha256(id)>.sealed`: one sealed file per secret ([`crate::crypto`]);
//! - `data/secrets/index.json`: names, labels, sizes and dates only, never values;
//! - `data/secrets/master.check`: a constant sealed under the master key, so a wrong passphrase or a replaced
//!   keyring entry is detected before anything is overwritten;
//! - `data/secrets/store.json`: which backend sealed the directory, so `auto` keeps choosing it;
//! - `data/secrets/master.salt`: the Argon2id salt (passphrase backend only).
//!
//! The directory is 0700 and every file 0600 (Unix). A lock file serializes writers across processes.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use russet_core::error::{Error, Result, codes};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::crypto::{self, MasterKey};
use crate::{BackendInfo, BackendKind, SecretBytes, SecretId, SecretKind, SecretMeta, SecretStore, check_size};

const INDEX_FILE: &str = "index.json";
const CHECK_FILE: &str = "master.check";
const STORE_FILE: &str = "store.json";
const LOCK_FILE: &str = ".lock";
const CHECK_PLAINTEXT: &[u8] = b"russet-master-check-v1";

/// Where the master key comes from: the OS keyring or a passphrase.
pub trait MasterKeySource: Send + Sync + 'static {
    /// The backend this source implements.
    fn kind(&self) -> BackendKind;
    /// The existing master key, or `None` when none was created yet.
    fn load(&self) -> Result<Option<MasterKey>>;
    /// Creates, persists and returns a new master key (only called while the store is empty).
    fn create(&self) -> Result<MasterKey>;
    /// The backend's state, probing the keyring when needed; never creates anything.
    fn probe(&self) -> BackendInfo;
    /// The state as last known, without I/O.
    fn describe(&self) -> BackendInfo;
    /// The keyring entry holding the key, for the keyring source.
    fn keyring_entry(&self) -> Option<String> {
        None
    }
}

/// `store.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoreFile {
    version: u32,
    backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keyring_entry: Option<String>,
}

/// One `index.json` entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexEntry {
    label: String,
    created_at: String,
    size_bytes: usize,
}

/// `index.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    entries: BTreeMap<String, IndexEntry>,
}

/// The backend name recorded in `store.json`.
pub fn backend_name(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::Keyring => "keyring",
        BackendKind::PassphraseFile => "passphrase",
        BackendKind::Env => "env",
        BackendKind::Memory => "memory",
    }
}

/// The backend that sealed `dir`, from its `store.json`.
pub fn sealed_by(dir: &Path) -> Option<BackendKind> {
    let text = std::fs::read_to_string(dir.join(STORE_FILE)).ok()?;
    let s: StoreFile = serde_json::from_str(&text).ok()?;
    match s.backend.as_str() {
        "keyring" => Some(BackendKind::Keyring),
        "passphrase" => Some(BackendKind::PassphraseFile),
        _ => None,
    }
}

struct Inner {
    dir: PathBuf,
    source: Box<dyn MasterKeySource>,
    key: Mutex<Option<MasterKey>>,
}

/// A store of sealed files under one master key.
#[derive(Clone)]
pub struct SealedStore {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SealedStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedStore").field("dir", &self.inner.dir).field("backend", &self.inner.source.kind()).finish()
    }
}

fn io_err(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::new(codes::core::IO, format!("cannot {what} {}: {e}", path.display()))
        .with_hint("Check the permissions of Russet's data directory.")
        .caused_by(e)
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

/// Creates `dir` 0700 (Unix).
pub(crate) fn ensure_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| io_err("create", dir, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| io_err("protect", dir, e))?;
    }
    Ok(())
}

/// Writes `bytes` to `path` atomically, mode 0600.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("secret");
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let res = (|| -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_err("write", path, e));
    }
    Ok(())
}

impl SealedStore {
    /// A store in `dir` (normally `data/secrets`) whose master key comes from `source`.
    pub fn new(dir: PathBuf, source: Box<dyn MasterKeySource>) -> Self {
        Self { inner: Arc::new(Inner { dir, source, key: Mutex::new(None) }) }
    }

    /// The store directory.
    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    fn file_of(&self, id: &SecretId) -> PathBuf {
        self.inner.dir.join(format!("{}.sealed", crypto::sha256_hex(id.to_string().as_bytes())))
    }

    fn lock_dir(&self) -> Result<std::fs::File> {
        ensure_private_dir(&self.inner.dir)?;
        let path = self.inner.dir.join(LOCK_FILE);
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let f = opts.open(&path).map_err(|e| io_err("open", &path, e))?;
        f.lock().map_err(|e| io_err("lock", &path, e))?;
        Ok(f)
    }

    fn read_index(&self) -> Result<IndexFile> {
        let path = self.inner.dir.join(INDEX_FILE);
        match std::fs::read_to_string(&path) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| {
                Error::new(codes::secrets::UNAVAILABLE, format!("{} is damaged: {e}", path.display()))
                    .with_hint("Restore the file from a backup, or remove the secrets directory and store them again.")
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(IndexFile { version: 1, ..Default::default() }),
            Err(e) => Err(io_err("read", &path, e)),
        }
    }

    fn write_index(&self, index: &IndexFile) -> Result<()> {
        let text = serde_json::to_vec_pretty(index).map_err(|e| {
            Error::new(codes::core::INTERNAL, format!("cannot encode the index: {e}")).with_hint("Report this bug.")
        })?;
        write_private(&self.inner.dir.join(INDEX_FILE), &text)
    }

    fn meta_of(&self, id: &str, e: &IndexEntry) -> Option<SecretMeta> {
        let id = SecretId::parse(id).ok()?;
        let created_at = OffsetDateTime::parse(&e.created_at, &Rfc3339).unwrap_or(OffsetDateTime::UNIX_EPOCH);
        Some(SecretMeta {
            id,
            label: e.label.clone(),
            backend: self.inner.source.kind(),
            created_at,
            size_bytes: e.size_bytes,
        })
    }

    /// The master key; created when `create` is set and the store holds nothing yet.
    fn master_key(&self, create: bool) -> Result<Option<MasterKey>> {
        let mut cached = match self.inner.key.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(k) = cached.as_ref() {
            return Ok(Some(k.clone()));
        }
        let check_path = self.inner.dir.join(CHECK_FILE);
        let check_id = check_id();
        let key = match self.inner.source.load()? {
            Some(k) => {
                match std::fs::read(&check_path) {
                    Ok(sealed) => {
                        crypto::open(&k, &check_id, &sealed).map_err(|_| self.key_mismatch())?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        if !self.read_index()?.entries.is_empty() {
                            return Err(self.key_mismatch());
                        }
                        if create {
                            ensure_private_dir(&self.inner.dir)?;
                            write_private(&check_path, &crypto::seal(&k, &check_id, CHECK_PLAINTEXT)?)?;
                            self.write_store_file()?;
                        }
                    }
                    Err(e) => return Err(io_err("read", &check_path, e)),
                }
                k
            }
            None => {
                if check_path.exists() || !self.read_index()?.entries.is_empty() {
                    return Err(Error::new(
                        codes::secrets::LOCKED,
                        format!(
                            "the master key of {} is missing, but secrets were sealed with one",
                            self.inner.dir.display()
                        ),
                    )
                    .with_hint(
                        "Restore the keyring entry named by `russet secrets status`, or remove the secrets \
                         directory and store the secrets again.",
                    ));
                }
                if !create {
                    return Ok(None);
                }
                ensure_private_dir(&self.inner.dir)?;
                let k = self.inner.source.create()?;
                write_private(&check_path, &crypto::seal(&k, &check_id, CHECK_PLAINTEXT)?)?;
                self.write_store_file()?;
                k
            }
        };
        *cached = Some(key.clone());
        Ok(Some(key))
    }

    /// Records which backend sealed the directory (`store.json`), so `auto` keeps choosing it.
    fn write_store_file(&self) -> Result<()> {
        let store = StoreFile {
            version: 1,
            backend: backend_name(self.inner.source.kind()).into(),
            keyring_entry: self.inner.source.keyring_entry(),
        };
        let text = serde_json::to_vec_pretty(&store).map_err(|e| {
            Error::new(codes::core::INTERNAL, format!("cannot encode store.json: {e}")).with_hint("Report this bug.")
        })?;
        write_private(&self.inner.dir.join(STORE_FILE), &text)
    }

    fn key_mismatch(&self) -> Error {
        let what = match self.inner.source.kind() {
            BackendKind::PassphraseFile => "the passphrase does not unlock",
            _ => "the keyring's master key does not open",
        };
        Error::new(codes::secrets::LOCKED, format!("{what} the secrets in {}", self.inner.dir.display())).with_hint(
            "Use the passphrase or keyring entry the secrets were sealed with (`russet secrets status` names the \
             entry), or remove the secrets directory and store the secrets again.",
        )
    }

    /// `put`, synchronously.
    pub fn put_sync(&self, id: &SecretId, label: &str, v: &SecretBytes) -> Result<SecretMeta> {
        check_size(v)?;
        let _lock = self.lock_dir()?;
        let key = self.master_key(true)?.ok_or_else(|| {
            Error::new(codes::core::INTERNAL, "no master key after creating one").with_hint("Report this bug.")
        })?;
        let sealed = crypto::seal(&key, id, v.expose())?;
        write_private(&self.file_of(id), &sealed)?;
        let mut index = self.read_index()?;
        let entry = IndexEntry { label: label.to_owned(), created_at: now_rfc3339(), size_bytes: v.len() };
        let meta = self.meta_of(&id.to_string(), &entry).ok_or_else(|| {
            Error::new(codes::core::INTERNAL, "cannot describe the secret").with_hint("Report this bug.")
        })?;
        index.version = 1;
        index.entries.insert(id.to_string(), entry);
        self.write_index(&index)?;
        Ok(meta)
    }

    /// `get`, synchronously.
    pub fn get_sync(&self, id: &SecretId) -> Result<Option<SecretBytes>> {
        let path = self.file_of(id);
        let sealed = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err("read", &path, e)),
        };
        let Some(key) = self.master_key(false)? else { return Ok(None) };
        crypto::open(&key, id, &sealed).map(Some)
    }

    /// `delete`, synchronously; needs no master key.
    pub fn delete_sync(&self, id: &SecretId) -> Result<bool> {
        if !self.inner.dir.exists() {
            return Ok(false);
        }
        let _lock = self.lock_dir()?;
        let path = self.file_of(id);
        let existed = match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(io_err("remove", &path, e)),
        };
        let mut index = self.read_index()?;
        let listed = index.entries.remove(&id.to_string()).is_some();
        if listed {
            self.write_index(&index)?;
        }
        Ok(existed || listed)
    }

    /// `list`, synchronously; reads names from `index.json` only.
    pub fn list_sync(&self, kind: Option<SecretKind>) -> Result<Vec<SecretMeta>> {
        let index = self.read_index()?;
        Ok(index
            .entries
            .iter()
            .filter_map(|(id, e)| self.meta_of(id, e))
            .filter(|m| kind.is_none_or(|k| m.id.kind == k))
            .collect())
    }

    /// The backend's state; probes the keyring (or checks the passphrase) without creating anything.
    pub fn status_sync(&self) -> BackendInfo {
        let mut info = self.inner.source.probe();
        if info.available
            && !info.locked
            && self.inner.dir.join(CHECK_FILE).exists()
            && let Err(e) = self.master_key(false)
        {
            info.locked = true;
            info.reason = Some(e.message().to_owned());
        }
        info
    }

    fn describe(&self) -> BackendInfo {
        self.inner.source.describe()
    }
}

fn check_id() -> SecretId {
    SecretId { kind: SecretKind::TcpToken, name: "\0russet-master-check".into() }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| {
        Error::new(codes::core::INTERNAL, format!("the secret-store task failed: {e}")).with_hint("Report this bug.")
    })?
}

#[async_trait]
impl SecretStore for SealedStore {
    async fn put(&self, id: &SecretId, label: &str, v: SecretBytes) -> Result<SecretMeta> {
        let (s, id, label) = (self.clone(), id.clone(), label.to_owned());
        blocking(move || s.put_sync(&id, &label, &v)).await
    }

    async fn get(&self, id: &SecretId) -> Result<Option<SecretBytes>> {
        let (s, id) = (self.clone(), id.clone());
        blocking(move || s.get_sync(&id)).await
    }

    async fn delete(&self, id: &SecretId) -> Result<bool> {
        let (s, id) = (self.clone(), id.clone());
        blocking(move || s.delete_sync(&id)).await
    }

    async fn list(&self, kind: Option<SecretKind>) -> Result<Vec<SecretMeta>> {
        let s = self.clone();
        blocking(move || s.list_sync(kind)).await
    }

    fn backend(&self) -> BackendInfo {
        self.describe()
    }

    fn keyring_entry(&self) -> Option<String> {
        self.inner.source.keyring_entry()
    }

    async fn status(&self) -> BackendInfo {
        let s = self.clone();
        match tokio::task::spawn_blocking(move || s.status_sync()).await {
            Ok(info) => info,
            Err(e) => BackendInfo { reason: Some(format!("the status task failed: {e}")), ..self.describe() },
        }
    }
}
