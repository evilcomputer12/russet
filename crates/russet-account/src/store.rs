// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Accounts (SIGN §6.1): `accounts.toml` in the config directory (ARCHITECTURE §14.4, machine-written,
//! kebab-case) plus the sealed keys, `account.addAscKey` and the one-time xtool import [D 50].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russet_asc::{AscClient, AscKey, Issuer, JwtAuth, RateLimiter, RetryPolicy, TokenFactory};
use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, codes};
use russet_secrets::{SecretBytes, SecretId, SecretKind, SecretStore};
use russet_sign::InMemoryIdentity;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::fsutil::{io_error, write_private};
use crate::identity::{IdentityStore, SigningIdentity};
use crate::portal::{AscPortal, DevPortal};
use crate::xtool;

/// An account id: `asc:<KEYID>` or `appleid:<DSID>` (ARCHITECTURE §14.4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(pub String);

impl AccountId {
    /// The id of an App Store Connect key.
    pub fn asc(key_id: &str) -> Self {
        Self(format!("asc:{key_id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What an account is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountKind {
    AscTeamKey,
    AscIndividualKey,
    AppleId,
}

impl AccountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AscTeamKey => "asc-team-key",
            Self::AscIndividualKey => "asc-individual-key",
            Self::AppleId => "apple-id",
        }
    }
}

/// One account from `accounts.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: AccountId,
    pub kind: AccountKind,
    pub label: String,
    pub key_id: Option<String>,
    pub issuer_id: Option<String>,
    /// Discovered from a certificate's subject OU, else a bundle ID's seed ID.
    pub team_id: Option<String>,
    /// `secret:asc-key/<KEYID>`.
    pub secret: String,
    /// `xtool` for the one-time import.
    pub imported_from: Option<String>,
    pub is_default: bool,
}

impl Account {
    /// The secret holding the key.
    pub fn secret_id(&self) -> Option<SecretId> {
        let rest = self.secret.strip_prefix("secret:")?;
        let (kind, name) = rest.split_once('/')?;
        let kind = match kind {
            "asc-key" => SecretKind::AscKey,
            "apple-id-session" => SecretKind::AppleIdSession,
            _ => return None,
        };
        Some(SecretId { kind, name: name.to_owned() })
    }

    /// The prefix xtool gives bundle IDs (`XTL-<prefix>.<id>`): the first segment of its identity ID, which for
    /// an App Store Connect login is the issuer ID, uppercased (build V12; xtool `ProvisioningIdentifiers`).
    pub fn xtool_prefix(&self) -> Option<String> {
        let issuer = self.issuer_id.as_deref()?;
        let first = issuer.split('-').next()?;
        (!first.is_empty()).then(|| first.to_uppercase())
    }
}

/// `account.addAscKey` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddAscKey {
    pub key_id: String,
    /// `None` for an individual key.
    pub issuer_id: Option<String>,
    pub p8_path: PathBuf,
    pub label: Option<String>,
}

/// What the one-time xtool import found or did [D 50].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XtoolImport {
    pub dry_run: bool,
    pub accounts: Vec<Account>,
    pub identities: Vec<SigningIdentity>,
    /// Advice and anything skipped: the `chmod 600` and `xtool auth logout` advice is always among them.
    pub warnings: Vec<String>,
}

/// The `accounts.toml` file.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct AccountsFile {
    #[serde(default = "one")]
    schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<String>,
    #[serde(default, rename = "account")]
    accounts: Vec<AccountEntry>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct AccountEntry {
    id: String,
    kind: AccountKind,
    #[serde(default)]
    label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    issuer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    team_id: Option<String>,
    secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    imported_from: Option<String>,
}

/// The rate limiters of the App Store Connect keys, by key ID: every client of a key shares one, so the hourly
/// quota spans calls (SIGN §5.3).
pub(crate) type Limiters = Arc<std::sync::Mutex<std::collections::BTreeMap<String, Arc<RateLimiter>>>>;

/// `accounts.toml` plus the sealed secrets.
pub struct AccountStore {
    path: PathBuf,
    file: AccountsFile,
    secrets: Arc<dyn SecretStore>,
    identities: IdentityStore,
    asc_base: Url,
    xtool_dir: Option<PathBuf>,
    limiters: Limiters,
}

impl std::fmt::Debug for AccountStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountStore")
            .field("path", &self.path)
            .field("accounts", &self.file.accounts.len())
            .finish_non_exhaustive()
    }
}

impl AccountStore {
    /// Loads `accounts.toml` (a missing file is an empty store).
    pub fn load(dirs: &RussetDirs, secrets: Arc<dyn SecretStore>) -> Result<Self> {
        let path = dirs.config.join("accounts.toml");
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<AccountsFile>(&text).map_err(|e| {
                Error::new(codes::config::INVALID, format!("{} is invalid: {}", path.display(), e.message()))
                    .with_hint("accounts.toml is written by `russet account …`; fix or remove the broken entry.")
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AccountsFile { schema: 1, ..Default::default() },
            Err(e) => return Err(io_error("cannot read", &path, e)),
        };
        let identities = IdentityStore::new(&dirs.data, secrets.clone());
        #[allow(clippy::unwrap_used)] // a constant, valid URL
        let asc_base = Url::parse(russet_asc::DEFAULT_BASE_URL).unwrap();
        Ok(Self {
            path,
            file,
            secrets,
            identities,
            asc_base,
            xtool_dir: xtool::default_dir(),
            limiters: Limiters::default(),
        })
    }

    /// The rate limiter every client of App Store Connect key `key_id` shares.
    pub fn limiter(&self, key_id: &str) -> Arc<RateLimiter> {
        let Ok(mut map) = self.limiters.lock() else { return Arc::new(RateLimiter::per_key()) };
        map.entry(key_id.to_owned()).or_insert_with(|| Arc::new(RateLimiter::per_key())).clone()
    }

    /// A GET-only client of `key` with per-request scopes, sharing the key's limiter.
    pub(crate) fn client(&self, key: &AscKey) -> Result<AscClient> {
        client_for(key.clone(), &self.asc_base, self.limiter(&key.key_id))
    }

    /// Points the App Store Connect client at `base` (`Endpoints.asc`; fakes in tests).
    pub fn with_asc_base(mut self, base: Url) -> Self {
        self.asc_base = base;
        self
    }

    /// Reads xtool's store from `dir` instead of `~/.config/xtool/data`.
    pub fn with_xtool_dir(mut self, dir: PathBuf) -> Self {
        self.xtool_dir = Some(dir);
        self
    }

    pub fn asc_base(&self) -> &Url {
        &self.asc_base
    }

    pub fn list(&self) -> Vec<Account> {
        self.file.accounts.iter().map(|e| self.to_account(e)).collect()
    }

    /// The default account: the one `default` names, else the only one.
    pub fn default_account(&self) -> Option<Account> {
        let id = self
            .file
            .default
            .clone()
            .or_else(|| (self.file.accounts.len() == 1).then(|| self.file.accounts[0].id.clone()))?;
        self.get(&AccountId(id)).ok()
    }

    pub fn get(&self, id: &AccountId) -> Result<Account> {
        self.file.accounts.iter().find(|e| e.id == id.0).map(|e| self.to_account(e)).ok_or_else(|| no_account(Some(id)))
    }

    /// The account `id`, or the default one.
    pub fn resolve(&self, id: Option<&AccountId>) -> Result<Account> {
        match id {
            Some(id) => self.get(id),
            None => self.default_account().ok_or_else(|| no_account(None)),
        }
    }

    fn to_account(&self, e: &AccountEntry) -> Account {
        let is_default = self.file.default.as_deref() == Some(e.id.as_str())
            || (self.file.default.is_none() && self.file.accounts.len() == 1);
        Account {
            id: AccountId(e.id.clone()),
            kind: e.kind,
            label: e.label.clone(),
            key_id: e.key_id.clone(),
            issuer_id: e.issuer_id.clone(),
            team_id: e.team_id.clone(),
            secret: e.secret.clone(),
            imported_from: e.imported_from.clone(),
            is_default,
        }
    }

    /// Adds (or replaces) an App Store Connect key: parses the `.p8`, checks it against App Store Connect with a
    /// GET-scoped token, discovers the team, seals the key and writes `accounts.toml`.
    pub async fn add_asc_key(&mut self, r: AddAscKey) -> Result<Account> {
        let pem = std::fs::read(&r.p8_path).map_err(|e| {
            Error::new(codes::account::KEY_INVALID, format!("cannot read {}: {e}", r.p8_path.display()))
                .with_hint("Pass the AuthKey_<KEYID>.p8 file App Store Connect gave you.")
        })?;
        let issuer = match &r.issuer_id {
            Some(i) => Issuer::Team { issuer_id: i.clone() },
            None => Issuer::Individual,
        };
        let key = AscKey::from_p8(&r.key_id, issuer, &pem)?;
        let team = discover_team(&self.client(&key)?, &key, &self.identities).await?;
        let kind = if key.is_individual() { AccountKind::AscIndividualKey } else { AccountKind::AscTeamKey };
        let label = r.label.unwrap_or_else(|| format!("App Store Connect key {}", r.key_id));
        self.seal_key(&r.key_id, &label, &pem).await?;
        let entry = AccountEntry {
            id: AccountId::asc(&r.key_id).0,
            kind,
            label,
            key_id: Some(r.key_id.clone()),
            issuer_id: r.issuer_id.clone(),
            team_id: team,
            secret: format!("secret:asc-key/{}", r.key_id),
            imported_from: None,
        };
        let id = AccountId(entry.id.clone());
        self.upsert(entry)?;
        self.get(&id)
    }

    /// The explicit, one-time import of xtool's App Store Connect key and development identity [D 50]. With
    /// `dry_run` nothing is sealed or written. The team comes from the identity certificate (no network).
    pub async fn import_xtool(&mut self, dry_run: bool) -> Result<XtoolImport> {
        let dir = self.xtool_dir.clone().ok_or_else(|| {
            Error::new(codes::core::NOT_FOUND, "cannot locate xtool's data directory (HOME is not set)")
                .with_hint("Set HOME (or XDG_CONFIG_HOME) to the user whose xtool login should be imported.")
        })?;
        let data = xtool::read(&dir)?;
        let mut warnings = data.warnings.clone();
        let mut accounts = Vec::new();
        let mut identities = Vec::new();

        let identity = match &data.identity {
            Some(x) => match InMemoryIdentity::from_pem(x.cert_der.clone(), &x.key_pem, None) {
                Ok(i) => Some(i),
                Err(e) => {
                    warnings.push(format!("xtool's development identity was not imported: {}", e.message()));
                    None
                }
            },
            None => None,
        };
        let team = identity.as_ref().map(|i| russet_sign::CodeSigningIdentity::team_id(i).to_owned());

        if let Some(k) = &data.asc_key {
            // Validate before sealing: a broken key must not land in the store.
            AscKey::from_p8(&k.key_id, Issuer::Team { issuer_id: k.issuer_id.clone() }, &k.pem)?;
            let label = format!("App Store Connect key {} (imported from xtool)", k.key_id);
            let entry = AccountEntry {
                id: AccountId::asc(&k.key_id).0,
                kind: AccountKind::AscTeamKey,
                label: label.clone(),
                key_id: Some(k.key_id.clone()),
                issuer_id: Some(k.issuer_id.clone()),
                team_id: team.clone(),
                secret: format!("secret:asc-key/{}", k.key_id),
                imported_from: Some("xtool".into()),
            };
            if dry_run {
                accounts.push(self.to_account(&entry));
            } else {
                self.seal_key(&k.key_id, &label, &k.pem).await?;
                let id = AccountId(entry.id.clone());
                self.upsert(entry)?;
                accounts.push(self.get(&id)?);
            }
        }
        if let Some(i) = &identity {
            if dry_run {
                let info = i.info()?;
                identities.push(SigningIdentity {
                    id: crate::identity::IdentityId::new(russet_sign::CodeSigningIdentity::team_id(i), &info.serial),
                    team_id: russet_sign::CodeSigningIdentity::team_id(i).to_owned(),
                    kind: crate::identity::CertKind::Development,
                    serial: info.serial,
                    common_name: info.common_name.unwrap_or_default(),
                    not_after: info.not_after,
                    label: "development identity (imported from xtool)".into(),
                    asc_certificate_id: None,
                    sha1: info.sha1,
                    imported_from: Some("xtool".into()),
                    cert_der: russet_sign::CodeSigningIdentity::certificate_der(i).to_vec(),
                });
            } else {
                identities.push(
                    self.identities.store(i, Some("development identity (imported from xtool)"), Some("xtool")).await?,
                );
            }
        }
        if accounts.is_empty() && identities.is_empty() {
            return Err(Error::new(codes::core::NOT_FOUND, "xtool's store holds nothing Russet can import")
                .with_hint(
                    "Log in to xtool with an App Store Connect key (`xtool auth`), or use `russet account add-key`.",
                )
                .with_details(serde_json::json!({"warnings": warnings})));
        }
        // The chmod 600 advice (SIGN §4.4, ARCHITECTURE §12.3), whatever the files' modes are today. It names
        // xtool's directory, never its files: the identity file is named after the App Store Connect issuer ID.
        let dir_shown = dir.display().to_string();
        if data.world_readable.is_empty() {
            warnings.push(format!(
                "xtool keeps its own copy of these credentials ({} file(s) in {dir_shown}); they are private (chmod 600) — keep them so, and run `xtool auth logout` once Russet provisions on its own (W12)",
                data.files.len()
            ));
        } else {
            warnings.push(format!(
                "{} of xtool's credential files in {dir_shown} are readable by other users: run `chmod 600 '{dir_shown}'/*`, and `xtool auth logout` once Russet provisions on its own (W12)",
                data.world_readable.len()
            ));
        }
        Ok(XtoolImport { dry_run, accounts, identities, warnings })
    }

    /// The developer portal of an account's team (GET-only in Wave 1).
    pub async fn portal(&self, id: &AccountId, team: Option<&str>) -> Result<Arc<dyn DevPortal>> {
        let account = self.get(id)?;
        let key = self.asc_key(&account).await?;
        let team_id = team.map(str::to_owned).or_else(|| account.team_id.clone()).ok_or_else(|| {
            Error::new(codes::account::NO_ACCOUNT, format!("the team of {id} is unknown"))
                .with_hint("Re-add the key (`russet account add-key …`) or import xtool's identity so Russet can discover the team; `russet account verify` shows what the key can read.")
        })?;
        Ok(Arc::new(AscPortal::new(self.client(&key)?, team_id, key_is_individual(&account))))
    }

    /// The account's App Store Connect key, unsealed (in-process only).
    pub async fn asc_key(&self, account: &Account) -> Result<AscKey> {
        let secret = account.secret_id().ok_or_else(|| {
            Error::new(codes::account::NO_ACCOUNT, format!("{} has no App Store Connect key", account.id))
                .with_hint("Only App Store Connect accounts read the portal in Wave 1: add a key with `russet account add-key`, or import xtool's with `russet account import --from-xtool --yes`.")
        })?;
        let pem = self.secrets.get(&secret).await?.ok_or_else(|| {
            Error::new(
                codes::account::NO_ACCOUNT,
                format!("the key of {} is missing from the secret store", account.id),
            )
            .with_hint("Add it again: `russet account import --from-xtool` or `russet account add-key`.")
        })?;
        let issuer = match (&account.kind, &account.issuer_id) {
            (AccountKind::AscTeamKey, Some(i)) => Issuer::Team { issuer_id: i.clone() },
            _ => Issuer::Individual,
        };
        AscKey::from_p8(account.key_id.as_deref().unwrap_or_default(), issuer, pem.expose())
    }

    /// Records a discovered team ID.
    pub fn set_team(&mut self, id: &AccountId, team: &str) -> Result<()> {
        let Some(e) = self.file.accounts.iter_mut().find(|e| e.id == id.0) else { return Err(no_account(Some(id))) };
        if e.team_id.as_deref() != Some(team) {
            e.team_id = Some(team.to_owned());
            self.save()?;
        }
        Ok(())
    }

    async fn seal_key(&self, key_id: &str, label: &str, pem: &[u8]) -> Result<()> {
        let id = SecretId { kind: SecretKind::AscKey, name: key_id.to_owned() };
        self.secrets.put(&id, label, SecretBytes::new(pem.to_vec())).await.map(|_| ())
    }

    fn upsert(&mut self, entry: AccountEntry) -> Result<()> {
        if let Some(e) = self.file.accounts.iter_mut().find(|e| e.id == entry.id) {
            *e = entry;
        } else {
            self.file.accounts.push(entry);
        }
        if self.file.default.is_none() && self.file.accounts.len() == 1 {
            self.file.default = Some(self.file.accounts[0].id.clone());
        }
        self.save()
    }

    fn save(&self) -> Result<()> {
        let body = toml::to_string_pretty(&self.file)
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode accounts.toml: {e}")))?;
        let text = format!(
            "# Written by `russet account …` (ARCHITECTURE §14.4). Keys and private keys live in the secret store.\n{body}"
        );
        write_private(&self.path, text.as_bytes())
    }

    /// Where the file lives.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn key_is_individual(a: &Account) -> bool {
    a.kind == AccountKind::AscIndividualKey
}

pub(crate) fn no_account(id: Option<&AccountId>) -> Error {
    let msg = match id {
        Some(id) => format!("no account {id}"),
        None => "no App Store Connect account is configured".into(),
    };
    Error::new(codes::account::NO_ACCOUNT, msg)
        .with_hint("Add one: `russet account import --from-xtool --yes` or `russet account add-key --key-id … --issuer-id … --p8 …`.")
}

/// A GET-only client with per-request scopes (Wave 1) that shares `limiter` with the key's other clients.
pub(crate) fn client_for(key: AscKey, base: &Url, limiter: Arc<RateLimiter>) -> Result<AscClient> {
    let auth = JwtAuth::per_request(base.clone(), Arc::new(TokenFactory::new(key)));
    AscClient::with_limiter(Arc::new(auth), RetryPolicy::default(), limiter)
}

/// Checks the key against App Store Connect with GET-scoped reads and discovers its team (SIGN §6.1 step 3).
///
/// - A team key reads `GET /v1/certificates?limit=…`: the team is a certificate's subject OU, else the team of a
///   local identity of one of those certificates, else the seed ID of a bundle ID. A 403 there means the key's
///   role cannot use the provisioning endpoints: the key is kept, without a team.
/// - An individual key cannot use the provisioning endpoints at all (SIGN §5.2), so it reads `GET /v1/apps` to
///   prove it is accepted, and has no team.
///
/// A 401 (wrong key, issuer or clock) is an error either way: nothing is stored.
pub(crate) async fn discover_team(
    client: &AscClient,
    key: &AscKey,
    identities: &IdentityStore,
) -> Result<Option<String>> {
    let forbidden =
        |e: &Error| e.code() == codes::account::ASC_FORBIDDEN || e.code() == codes::account::INSUFFICIENT_ROLE;
    if key.is_individual() {
        return match client.get_json::<serde_json::Value>("/v1/apps?limit=1").await {
            Ok(_) => Ok(None),
            Err(e) if forbidden(&e) => Ok(None),
            Err(e) => Err(e),
        };
    }
    let certs = match client.certificates().probe(20).await {
        Ok(c) => c,
        Err(e) if forbidden(&e) => Vec::new(),
        Err(e) => return Err(e),
    };
    for c in &certs {
        let Some(content) = c.attributes.as_ref().and_then(|a| a.certificate_content.as_deref()) else { continue };
        let Some(der) = base64_std(content) else { continue };
        if let Ok(info) = russet_sign::certificate_info(&der)
            && let Some(team) = info.team_id
        {
            return Ok(Some(team));
        }
    }
    let serials: Vec<String> = certs.iter().filter_map(|c| c.attributes.as_ref()?.serial_number.clone()).collect();
    if let Some(i) = identities.list(None)?.into_iter().find(|i| serials.contains(&i.serial)) {
        return Ok(Some(i.team_id));
    }
    let bundles = client.bundle_ids().list(None).await.unwrap_or_default();
    Ok(bundles.iter().find_map(|b| b.resource.attributes.as_ref()?.seed_id.clone()))
}

/// Standard base64 decoding (App Store Connect's `certificateContent` and `profileContent`).
pub(crate) fn base64_std(s: &str) -> Option<Vec<u8>> {
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let url_safe: String = clean
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            other => other,
        })
        .collect();
    russet_asc::b64url_decode(&url_safe)
}
