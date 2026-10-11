// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Signing identities (SIGN §6.3): the public part on disk (`data/identities/<team>/<serial>.{json,cer}`,
//! ARCHITECTURE §14.8), the private key sealed in the secret store as `signing-key/<certificate SHA-1>` (SIGN §5
//! names it `<team>/<serial>`, but `secrets.list` shows secret names, and they must not name the team).
//!
//! The [`IdentityId`] (`<team>/<serial>`) stays inside the daemon. The wire names an identity by its certificate
//! serial ([`IdentityId::wire`]), and [`IdentityStore::resolve`] maps a serial, a SHA-1 fingerprint or an
//! [`IdentityId`] back; messages show [`IdentityId::masked`]. No team ID leaves the daemon unmasked (AGENTS.md §8).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_secrets::{SecretBytes, SecretId, SecretKind, SecretStore};
use russet_sign::{CertificateKind, CodeSigningIdentity, InMemoryIdentity, KeyInfoSigner, certificate_info};
use serde::{Deserialize, Serialize};

use crate::fsutil::{io_error, parse_rfc3339, rfc3339, shown, write_private};

/// A signing identity id: `<team>/<serial>`, internal to the daemon (its team part is a team ID). The wire form is
/// [`IdentityId::wire`]; messages use [`IdentityId::masked`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IdentityId(pub String);

impl IdentityId {
    pub fn new(team: &str, serial: &str) -> Self {
        Self(format!("{team}/{serial}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `(team, serial)`.
    pub fn parts(&self) -> Option<(&str, &str)> {
        self.0.split_once('/')
    }

    /// The identity's name on the wire: its certificate serial (uppercase hex without leading zeros, as the
    /// portal's `serialNumber` and `russet account certs list` show it). It names no team.
    pub fn wire(&self) -> String {
        self.parts().map_or_else(|| self.0.clone(), |(_, serial)| serial.to_owned())
    }

    /// `*******345/<serial>`, for messages.
    pub fn masked(&self) -> String {
        match self.parts() {
            Some((team, serial)) => format!("{}/{serial}", russet_sign::mask_team(team)),
            None => self.0.clone(),
        }
    }
}

/// Shows the masked form ([`IdentityId::masked`]): the raw id holds a team ID.
impl std::fmt::Display for IdentityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.masked())
    }
}

/// Certificate kind for a new CSR (and of an identity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CertKind {
    Development,
    Distribution,
}

impl CertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Distribution => "distribution",
        }
    }
}

/// A signing identity's public part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningIdentity {
    pub id: IdentityId,
    pub team_id: String,
    pub kind: CertKind,
    /// Uppercase hex without leading zeros (the portal's `serialNumber`).
    pub serial: String,
    pub common_name: String,
    pub not_after: SystemTime,
    pub label: String,
    /// The portal's certificate id, once known.
    pub asc_certificate_id: Option<String>,
    /// Lower-case hex SHA-1 of the certificate.
    pub sha1: String,
    /// `xtool` for the one-time import [D 50].
    pub imported_from: Option<String>,
    /// The public certificate (DER).
    pub cert_der: Vec<u8>,
}

impl SigningIdentity {
    /// Whether the certificate has expired.
    pub fn is_expired(&self) -> bool {
        self.not_after <= SystemTime::now()
    }

    /// The secret holding the private key.
    pub fn secret_id(&self) -> SecretId {
        let name = if self.sha1.is_empty() { self.id.wire() } else { self.sha1.clone() };
        SecretId { kind: SecretKind::SigningKey, name }
    }
}

/// The JSON file next to the `.cer`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityFile {
    schema: u32,
    id: String,
    team_id: String,
    kind: CertKind,
    serial: String,
    common_name: String,
    not_after: String,
    label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asc_certificate_id: Option<String>,
    sha1: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    imported_from: Option<String>,
    secret: String,
}

/// A loaded identity: certificate plus private key; implements [`CodeSigningIdentity`].
pub struct LoadedIdentity {
    pub identity: SigningIdentity,
    inner: InMemoryIdentity,
}

impl std::fmt::Debug for LoadedIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedIdentity").field("id", &self.identity.id).finish_non_exhaustive()
    }
}

impl LoadedIdentity {
    /// The in-memory identity (for callers that hold it across a blocking task).
    pub fn into_inner(self) -> InMemoryIdentity {
        self.inner
    }
}

impl CodeSigningIdentity for LoadedIdentity {
    fn certificate_der(&self) -> &[u8] {
        self.inner.certificate_der()
    }
    fn team_id(&self) -> &str {
        self.inner.team_id()
    }
    fn signer(&self) -> &dyn KeyInfoSigner {
        self.inner.signer()
    }
}

/// A private key generated for a CSR, before the certificate exists (W12).
#[derive(Debug)]
pub struct PendingKey {
    _f0: (),
}

/// Signing identities (public parts on disk, private keys sealed).
#[derive(Clone)]
pub struct IdentityStore {
    dir: PathBuf,
    secrets: Arc<dyn SecretStore>,
}

impl std::fmt::Debug for IdentityStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityStore").field("dir", &self.dir).finish_non_exhaustive()
    }
}

impl IdentityStore {
    /// The store under `data_dir/identities`.
    pub fn new(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> Self {
        Self { dir: data_dir.join("identities"), secrets }
    }

    /// Every identity, optionally of one team, newest certificate first. `team` is a team ID or its masked form
    /// (`*******345`, what the wire shows).
    pub fn list(&self, team: Option<&str>) -> Result<Vec<SigningIdentity>> {
        let mut out = Vec::new();
        let Ok(teams) = std::fs::read_dir(&self.dir) else { return Ok(out) };
        for t in teams.filter_map(|e| e.ok()) {
            let team_dir = t.path();
            let name = t.file_name().to_string_lossy().into_owned();
            if !team_dir.is_dir() || team.is_some_and(|want| name != want && russet_sign::mask_team(&name) != want) {
                continue;
            }
            let entries = std::fs::read_dir(&team_dir).map_err(|e| io_error("cannot read", &team_dir, e))?;
            for f in entries.filter_map(|e| e.ok()) {
                let p = f.path();
                if p.extension().is_some_and(|e| e == "json") {
                    out.push(read_identity(&p)?);
                }
            }
        }
        out.sort_by(|a, b| b.not_after.cmp(&a.not_after).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// One identity's public part.
    pub fn get(&self, id: &IdentityId) -> Result<SigningIdentity> {
        let (team, serial) = id.parts().ok_or_else(|| invalid_id(id))?;
        if !safe_component(team) || !safe_component(serial) {
            return Err(invalid_id(id));
        }
        let path = self.dir.join(team).join(format!("{serial}.json"));
        if !path.is_file() {
            return Err(Error::new(codes::sign::NO_IDENTITY, format!("no signing identity {id}")).with_hint(
                "List them with `russet account list`; import one with `russet account import --from-xtool`.",
            ));
        }
        read_identity(&path)
    }

    /// The identity a client names: its certificate serial (the wire form, [`IdentityId::wire`]; leading zeros
    /// and case ignored), its SHA-1 fingerprint (40 hex digits, what `security find-identity` shows), or an
    /// [`IdentityId`] (`<team>/<serial>`).
    pub fn resolve(&self, name: &str) -> Result<SigningIdentity> {
        let name = name.trim();
        if name.contains('/') {
            return self.get(&IdentityId(name.to_owned()));
        }
        let wanted = name.to_ascii_uppercase();
        let serial = wanted.trim_start_matches('0');
        let mut matches: Vec<SigningIdentity> = self
            .list(None)?
            .into_iter()
            .filter(|i| i.serial.eq_ignore_ascii_case(serial) || i.sha1.eq_ignore_ascii_case(name))
            .collect();
        match matches.len() {
            1 => Ok(matches.remove(0)),
            0 => Err(Error::new(codes::sign::NO_IDENTITY, format!("no signing identity {name}")).with_hint(
                "Name an identity by the id `russet account list` shows (its certificate serial) or its SHA-1.",
            )),
            n => Err(Error::new(
                codes::core::INVALID_ARGUMENT,
                format!("{n} signing identities have the certificate serial {name}"),
            )
            .with_hint("Name the identity by its SHA-1 fingerprint (the sha1 field of `russet account list --json`).")),
        }
    }

    /// Loads an identity with its private key (from the secret store; in-process only). ARCHITECTURE §5.3's
    /// synchronous form: the secret store is asynchronous, so the read runs on a short-lived thread with its own
    /// runtime. It may be called from a runtime worker (it blocks that worker for the read) or from a blocking
    /// thread; async callers can use [`Self::load_async`].
    pub fn load(&self, id: &IdentityId) -> Result<LoadedIdentity> {
        block_on_secrets(self.load_async(id))
    }

    /// [`Self::load`] for async callers.
    pub async fn load_async(&self, id: &IdentityId) -> Result<LoadedIdentity> {
        let identity = self.get(id)?;
        let key = self.secrets.get(&identity.secret_id()).await?.ok_or_else(|| {
            Error::new(codes::sign::NO_IDENTITY, format!("the private key of {id} is not in the secret store"))
                .with_hint("Import the identity again (`russet account import --from-xtool` or a .p12).")
        })?;
        let inner =
            InMemoryIdentity::from_pkcs8(identity.cert_der.clone(), key.expose(), Some(identity.team_id.clone()))?;
        Ok(LoadedIdentity { identity, inner })
    }

    /// Imports a PKCS#12 identity (`identity.importP12` is W12's RPC; the store method is shared). Synchronous as
    /// in ARCHITECTURE §5.3, like [`Self::load`]; async callers use [`Self::import_p12_async`].
    pub fn import_p12(&self, p12: &[u8], pw: &SecretBytes) -> Result<SigningIdentity> {
        block_on_secrets(self.import_p12_async(p12, pw))
    }

    /// [`Self::import_p12`] for async callers.
    pub async fn import_p12_async(&self, p12: &[u8], pw: &SecretBytes) -> Result<SigningIdentity> {
        let pw = std::str::from_utf8(pw.expose())
            .map_err(|_| Error::new(codes::core::INVALID_ARGUMENT, "the .p12 password is not UTF-8"))?;
        let identity = InMemoryIdentity::from_p12(p12, pw, None)?;
        self.store(&identity, None, None).await
    }

    /// Returns the pending key and the PEM CSR (W12).
    pub fn new_csr(&self, _team: &str, _k: CertKind) -> Result<(PendingKey, String)> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "creating certificates is not available yet")
            .with_hint("Wave 1 reuses an existing identity: `russet account import --from-xtool`. Certificate creation lands in W12."))
    }

    /// Seals the key and writes the public part. Replaces an identity with the same id.
    pub async fn store(
        &self,
        identity: &InMemoryIdentity,
        label: Option<&str>,
        imported_from: Option<&str>,
    ) -> Result<SigningIdentity> {
        let info = identity.info()?;
        let team = identity.team_id().to_owned();
        if !safe_component(&team) || !safe_component(&info.serial) {
            return Err(Error::new(
                codes::sign::NO_IDENTITY,
                "the certificate's team or serial is not usable as a name",
            )
            .with_hint(
                "Apple signing certificates carry a 10-character team (subject OU) and a hex serial; use one of those.",
            ));
        }
        let kind = match info.kind {
            CertificateKind::Distribution => CertKind::Distribution,
            _ => CertKind::Development,
        };
        let id = IdentityId::new(&team, &info.serial);
        let label = label.map(str::to_owned).unwrap_or_else(|| {
            format!("{} certificate {}", kind.as_str(), &info.serial[info.serial.len().saturating_sub(6)..])
        });
        let signing = SigningIdentity {
            id: id.clone(),
            team_id: team.clone(),
            kind,
            serial: info.serial.clone(),
            common_name: info.common_name.clone().unwrap_or_default(),
            not_after: info.not_after,
            label,
            asc_certificate_id: None,
            sha1: info.sha1.clone(),
            imported_from: imported_from.map(str::to_owned),
            cert_der: identity.certificate_der().to_vec(),
        };
        let key = identity.private_key_pkcs8()?;
        self.secrets
            .put(&signing.secret_id(), &format!("Signing key {}", signing.label), SecretBytes::new(key.to_vec()))
            .await?;
        let file = IdentityFile {
            schema: 1,
            id: id.0.clone(),
            team_id: team.clone(),
            kind,
            serial: info.serial.clone(),
            common_name: signing.common_name.clone(),
            not_after: rfc3339(info.not_after),
            label: signing.label.clone(),
            asc_certificate_id: None,
            sha1: info.sha1,
            imported_from: signing.imported_from.clone(),
            secret: format!("secret:{}", signing.secret_id()),
        };
        let base = self.dir.join(&team);
        let json = serde_json::to_vec_pretty(&file)
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode the identity: {e}")))?;
        write_private(&base.join(format!("{}.cer", info.serial)), &signing.cert_der)?;
        write_private(&base.join(format!("{}.json", info.serial)), &json)?;
        Ok(signing)
    }
}

/// Runs a secret-store future to completion from synchronous code: on a scoped thread with its own
/// current-thread runtime, so it works inside and outside a Tokio runtime (the stores use `spawn_blocking` and
/// their own keyring threads, never the caller's runtime).
fn block_on_secrets<T: Send>(fut: impl Future<Output = Result<T>> + Send) -> Result<T> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| {
                    Error::new(codes::core::INTERNAL, format!("cannot start a runtime for the secret store: {e}"))
                })?;
                rt.block_on(fut)
            })
            .join()
            .map_err(|_| Error::new(codes::core::INTERNAL, "the secret-store read panicked"))?
    })
}

fn invalid_id(id: &IdentityId) -> Error {
    Error::new(codes::core::INVALID_ARGUMENT, format!("invalid identity id `{id}`"))
        .with_hint("Name an identity by the id `russet account list` shows (its certificate serial) or its SHA-1.")
}

/// A path component that cannot escape its directory.
fn safe_component(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn read_identity(json_path: &Path) -> Result<SigningIdentity> {
    let raw = std::fs::read(json_path).map_err(|e| io_error("cannot read", json_path, e))?;
    let f: IdentityFile = serde_json::from_slice(&raw).map_err(|e| {
        Error::new(codes::core::INVALID_ARGUMENT, format!("{} is not an identity file: {e}", shown(json_path)))
            .with_hint("Remove the broken file and import the identity again (`russet account import --from-xtool`).")
    })?;
    let cer = json_path.with_extension("cer");
    let cert_der = std::fs::read(&cer).map_err(|e| io_error("cannot read", &cer, e))?;
    // The certificate is the source of truth for expiry; the JSON is a cache of it.
    let not_after = certificate_info(&cert_der).map(|i| i.not_after).ok().or_else(|| parse_rfc3339(&f.not_after));
    Ok(SigningIdentity {
        id: IdentityId(f.id),
        team_id: f.team_id,
        kind: f.kind,
        serial: f.serial,
        common_name: f.common_name,
        not_after: not_after.unwrap_or(SystemTime::UNIX_EPOCH),
        label: f.label,
        asc_certificate_id: f.asc_certificate_id,
        sha1: f.sha1,
        imported_from: f.imported_from,
        cert_der,
    })
}
