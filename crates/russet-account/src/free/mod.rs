// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Apple ID accounts (W22; SIGN §7): the native sign-in behind `account.appleId.*`, the anisette provider of an
//! account, and the developer-services portal of an Apple ID team. Paid personal teams come first; free teams
//! use the same path (their 7-day policy, SA-21, is [`policy`]).
//!
//! [`AppleIds::begin`] signs in with Russet's own GrandSlam client and either finishes (no second factor) or
//! keeps a pending session for [`AppleIds::submit_code`]; a trusted-device code is requested right away. On
//! success the Xcode token and developer team are fetched, the session is sealed as
//! `secret:apple-id-session/<DSID>` (never the password) and the account is written to `accounts.toml`.
//! Pending sessions live in memory for 10 minutes and hold the password only until the second login.
//!
//! Nothing here logs or returns the Apple ID, password, codes or tokens; the account label masks the Apple ID.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russet_anisette::libs::{self, Consent, Fetched};
use russet_anisette::local::{AdiBackend, LocalAdi, ProcessBackend, adi_dir, default_host_exe};
use russet_anisette::{AnisetteProvider, RemoteV3};
use russet_asc::ds2::{DS2_BASE_URL, Ds2Auth};
use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, codes};
use russet_core::net::Endpoints;
use russet_gsa::{
    ClientProfile, Ds2Session, GsSession, GsaClient, LoginState, PendingTwoFactor, Qh65b2, SecondFactor, StoredSession,
    TeamTier, TrustedNumber, TwoFactorVia,
};
use russet_secrets::{SecretBytes, SecretId, SecretKind, SecretStore};
use serde::{Deserialize, Serialize};
use url::Url;
use zeroize::Zeroizing;

use crate::portal::{AscPortal, DevPortal};
use crate::store::{Account, AccountId, AccountStore};

pub mod policy;
#[cfg(test)]
mod tests;

/// How long a sign-in waits for its two-factor code.
pub const LOGIN_TTL: Duration = Duration::from_secs(600);

/// Wrong codes accepted before the sign-in session is dropped.
pub const MAX_CODE_ATTEMPTS: u32 = 5;

/// `[[account]] anisette` (ARCHITECTURE §14.4): `"local"` or `{ remote = "https://…" }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnisetteSetting {
    #[default]
    Local,
    Remote(String),
}

/// The Apple ID fields of an `[[account]]`.
#[derive(Clone, PartialEq, Eq)]
pub struct AppleIdEntry {
    pub dsid: String,
    /// The Apple ID (kept in `accounts.toml`, 0600; never logged or sent to clients).
    pub apple_id: String,
    pub label: String,
    pub team_id: Option<String>,
    /// `paid` or `free`.
    pub team_tier: Option<String>,
    pub anisette: AnisetteSetting,
}

impl std::fmt::Debug for AppleIdEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleIdEntry").field("team_tier", &self.team_tier).finish_non_exhaustive()
    }
}

impl AppleIdEntry {
    pub fn account_id(&self) -> AccountId {
        AccountId(format!("appleid:{}", self.dsid))
    }

    /// The sealed session.
    pub fn secret_id(&self) -> SecretId {
        SecretId { kind: SecretKind::AppleIdSession, name: self.dsid.clone() }
    }
}

/// `so…@ex…` — enough for the user to recognise their Apple ID, not enough to name it.
pub fn masked_apple_id(apple_id: &str) -> String {
    let (user, domain) = apple_id.split_once('@').unwrap_or((apple_id, ""));
    let head = |s: &str| s.chars().take(2).collect::<String>();
    if domain.is_empty() { format!("{}…", head(user)) } else { format!("{}…@{}…", head(user), head(domain)) }
}

/// What [`AppleIds::begin`] reached.
#[derive(Debug)]
pub enum BeginOutcome {
    Done { session_id: String, account: Account },
    TwoFactor { session_id: String, info: TwoFactorStep },
}

/// The two-factor step a client shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorStep {
    /// Apple offers trusted-device pushes (else SMS only).
    pub trusted_device: bool,
    /// A push went to the trusted devices already.
    pub pushed: bool,
    pub phone_numbers: Vec<TrustedNumber>,
}

/// What `account.anisette.status` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnisetteStatus {
    pub libraries_present: bool,
    pub apk_version: Option<String>,
    pub pinned: Option<bool>,
    pub provisioned: bool,
    pub local_supported: bool,
}

struct Pending {
    gsa: Arc<GsaClient>,
    pending: PendingTwoFactor,
    apple_id: Zeroizing<String>,
    password: Zeroizing<Vec<u8>>,
    via: TwoFactorVia,
    label: Option<String>,
    anisette: AnisetteSetting,
    created: Instant,
    attempts: u32,
}

/// The Apple ID service of one Russet home (`AppleIds::shared`).
pub struct AppleIds {
    dirs: RussetDirs,
    secrets: Arc<dyn SecretStore>,
    endpoints: Endpoints,
    ds2_base: Url,
    backend: Mutex<Option<Arc<dyn AdiBackend>>>,
    anisette_override: Mutex<Option<Arc<dyn AnisetteProvider>>>,
    local: tokio::sync::Mutex<Option<Arc<dyn AnisetteProvider>>>,
    logins: Mutex<BTreeMap<String, Pending>>,
}

impl std::fmt::Debug for AppleIds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleIds").field("config", &self.dirs.config).finish_non_exhaustive()
    }
}

/// The services by config directory: the daemon's handlers and its account store share one per Russet home.
static SHARED: Mutex<BTreeMap<PathBuf, Arc<AppleIds>>> = Mutex::new(BTreeMap::new());

fn login_expired() -> Error {
    Error::new(codes::gsa::LOGIN_EXPIRED, "this sign-in session is unknown or has expired")
        .with_hint("Start the sign-in again (sessions last 10 minutes and end after too many wrong codes).")
}

fn poisoned() -> Error {
    Error::new(codes::core::INTERNAL, "the Apple ID sign-in table is poisoned")
}

impl AppleIds {
    pub fn new(dirs: &RussetDirs, secrets: Arc<dyn SecretStore>, endpoints: Endpoints) -> Self {
        #[allow(clippy::unwrap_used)] // a constant, valid URL
        let ds2_base = Url::parse(DS2_BASE_URL).unwrap();
        Self {
            dirs: dirs.clone(),
            secrets,
            endpoints,
            ds2_base,
            backend: Mutex::new(None),
            anisette_override: Mutex::new(None),
            local: tokio::sync::Mutex::new(None),
            logins: Mutex::new(BTreeMap::new()),
        }
    }

    /// The service of the Russet home `dirs` (created on first use).
    pub fn shared(dirs: &RussetDirs, secrets: Arc<dyn SecretStore>, endpoints: Endpoints) -> Arc<Self> {
        let Ok(mut map) = SHARED.lock() else { return Arc::new(Self::new(dirs, secrets, endpoints)) };
        map.entry(dirs.config.clone()).or_insert_with(|| Arc::new(Self::new(dirs, secrets, endpoints))).clone()
    }

    /// Registers `svc` as the service of its Russet home (tests that inject fakes).
    pub fn register(svc: Arc<Self>) {
        if let Ok(mut map) = SHARED.lock() {
            map.insert(svc.dirs.config.clone(), svc);
        }
    }

    /// The service registered for the config directory `config`, if any.
    pub fn lookup(config: &std::path::Path) -> Option<Arc<Self>> {
        SHARED.lock().ok()?.get(config).cloned()
    }

    /// Points developerservices2 at `base` (a fake's).
    pub fn with_ds2_base(mut self, base: Url) -> Self {
        self.ds2_base = base;
        self
    }

    /// Runs local ADI on `backend` instead of `russet-adi-host` (the mocked suites' `FakeAdi`).
    pub fn with_adi_backend(self, backend: Arc<dyn AdiBackend>) -> Self {
        if let Ok(mut b) = self.backend.lock() {
            *b = Some(backend);
        }
        self
    }

    /// Uses `provider` for every sign-in (the mocked suites' `AnisetteFake`).
    pub fn with_anisette(self, provider: Arc<dyn AnisetteProvider>) -> Self {
        if let Ok(mut a) = self.anisette_override.lock() {
            *a = Some(provider);
        }
        self
    }

    async fn gsa_http(&self) -> Result<reqwest::Client> {
        russet_anisette::apple_tls::gsa_client(&self.endpoints.gsa, &self.dirs.data).await
    }

    async fn provider(&self, setting: &AnisetteSetting, profile: &ClientProfile) -> Result<Arc<dyn AnisetteProvider>> {
        if let Some(p) = self.anisette_override.lock().map_err(|_| poisoned())?.clone() {
            return Ok(p);
        }
        match setting {
            AnisetteSetting::Local => self.local_provider(profile).await,
            AnisetteSetting::Remote(url) => {
                let url = Url::parse(url)
                    .map_err(|e| Error::invalid_argument(format!("the anisette server URL is invalid: {e}")))?;
                let remote = RemoteV3::new(
                    url,
                    self.secrets.clone(),
                    self.gsa_http().await?,
                    self.endpoints.gsa.clone(),
                    profile.identity(),
                )?;
                Ok(Arc::new(remote))
            }
        }
    }

    async fn local_provider(&self, profile: &ClientProfile) -> Result<Arc<dyn AnisetteProvider>> {
        let mut cached = self.local.lock().await;
        if let Some(p) = cached.clone() {
            return Ok(p);
        }
        let backend = self.backend.lock().map_err(|_| poisoned())?.clone();
        if backend.is_none() && !russet_anisette::local_supported() {
            return Err(Error::new(codes::anisette::UNSUPPORTED, "local anisette does not run on this host")
                .with_hint("Russet runs local anisette in its Linux engine (the WSL backend on Windows); or sign in with a remote anisette-v3 server."));
        }
        let abi = libs::Abi::host()
            .ok_or_else(|| Error::new(codes::anisette::UNSUPPORTED, "no ADI libraries exist for this CPU"))?;
        let set = libs::installed(&self.dirs.data, abi).ok_or_else(libs::libs_missing)?;
        let adi = adi_dir(&self.dirs.data);
        let backend: Arc<dyn AdiBackend> = match backend {
            Some(b) => b,
            None => {
                let exe = default_host_exe().ok_or_else(|| {
                    Error::new(codes::anisette::ADI_FAILED, "the anisette helper (russet-adi-host) is not installed")
                        .with_hint("Reinstall Russet: russet-adi-host ships next to russetd (or set RUSSET_ADI_HOST).")
                })?;
                Arc::new(ProcessBackend::new(exe, &set.dir, &adi))
            }
        };
        let provider: Arc<dyn AnisetteProvider> = Arc::new(LocalAdi::new(
            set,
            backend,
            self.secrets.clone(),
            self.gsa_http().await?,
            self.endpoints.gsa.clone(),
            profile.identity(),
            adi,
        ));
        *cached = Some(provider.clone());
        Ok(provider)
    }

    async fn client(&self, setting: &AnisetteSetting) -> Result<Arc<GsaClient>> {
        let profile = ClientProfile::load(&self.dirs.config)?;
        let provider = self.provider(setting, &profile).await?;
        Ok(Arc::new(GsaClient::new(self.gsa_http().await?, self.endpoints.gsa.clone(), provider, profile)))
    }

    /// Signs in (`account.appleId.begin`).
    pub async fn begin(
        &self,
        accounts: &tokio::sync::RwLock<AccountStore>,
        apple_id: &str,
        password: SecretBytes,
        anisette: AnisetteSetting,
        label: Option<String>,
    ) -> Result<BeginOutcome> {
        if apple_id.trim().is_empty() || password.is_empty() {
            return Err(Error::invalid_argument("an Apple ID and a password are required"));
        }
        let gsa = self.client(&anisette).await?;
        let session_id = uuid_v4();
        match gsa.login(apple_id, password.expose()).await? {
            LoginState::Authenticated(s) => {
                let account = self.finish(accounts, &gsa, &s, apple_id, label, anisette).await?;
                Ok(BeginOutcome::Done { session_id, account })
            }
            LoginState::NeedsTwoFactor(pending) => {
                let trusted_device = pending.kind == SecondFactor::TrustedDevice;
                let pushed = trusted_device && gsa.request_code(&pending, TwoFactorVia::TrustedDevice).await.is_ok();
                let phone_numbers = gsa.trusted_numbers(&pending).await.unwrap_or_default();
                let via = match (trusted_device, phone_numbers.as_slice()) {
                    (true, _) => TwoFactorVia::TrustedDevice,
                    (false, [only]) => TwoFactorVia::Sms { phone_id: only.id },
                    _ => TwoFactorVia::TrustedDevice,
                };
                let mut logins = self.logins.lock().map_err(|_| poisoned())?;
                logins.retain(|_, p| p.created.elapsed() < LOGIN_TTL);
                logins.insert(
                    session_id.clone(),
                    Pending {
                        gsa,
                        pending,
                        apple_id: Zeroizing::new(apple_id.to_owned()),
                        password: Zeroizing::new(password.expose().to_vec()),
                        via,
                        label,
                        anisette,
                        created: Instant::now(),
                        attempts: 0,
                    },
                );
                Ok(BeginOutcome::TwoFactor {
                    session_id,
                    info: TwoFactorStep { trusted_device, pushed, phone_numbers },
                })
            }
        }
    }

    fn take_parts(&self, session_id: &str) -> Result<(Arc<GsaClient>, PendingTwoFactor)> {
        let mut logins = self.logins.lock().map_err(|_| poisoned())?;
        logins.retain(|_, p| p.created.elapsed() < LOGIN_TTL);
        let p = logins.get(session_id).ok_or_else(login_expired)?;
        Ok((p.gsa.clone(), p.pending.clone()))
    }

    /// Requests a code (`account.appleId.requestCode`); later codes are checked against this route.
    pub async fn request_code(&self, session_id: &str, via: TwoFactorVia) -> Result<()> {
        let (gsa, pending) = self.take_parts(session_id)?;
        gsa.request_code(&pending, via).await?;
        let mut logins = self.logins.lock().map_err(|_| poisoned())?;
        if let Some(p) = logins.get_mut(session_id) {
            p.via = via;
        }
        Ok(())
    }

    /// Checks a code and finishes the sign-in (`account.appleId.submitCode`).
    pub async fn submit_code(
        &self,
        accounts: &tokio::sync::RwLock<AccountStore>,
        session_id: &str,
        code: &str,
    ) -> Result<Account> {
        let (gsa, pending) = self.take_parts(session_id)?;
        let via =
            self.logins.lock().map_err(|_| poisoned())?.get(session_id).map(|p| p.via).ok_or_else(login_expired)?;
        if let Err(e) = gsa.submit_code(&pending, via, code).await {
            let mut logins = self.logins.lock().map_err(|_| poisoned())?;
            if let Some(p) = logins.get_mut(session_id) {
                p.attempts += 1;
                if p.attempts >= MAX_CODE_ATTEMPTS || e.code() == codes::gsa::CODE_RATE_LIMITED {
                    logins.remove(session_id);
                }
            }
            return Err(e);
        }
        let p = self.logins.lock().map_err(|_| poisoned())?.remove(session_id).ok_or_else(login_expired)?;
        match gsa.login(&p.apple_id, &p.password).await? {
            LoginState::Authenticated(s) => self.finish(accounts, &gsa, &s, &p.apple_id, p.label, p.anisette).await,
            LoginState::NeedsTwoFactor(_) => {
                Err(Error::new(codes::gsa::PROTOCOL, "Apple still asks for a second factor after accepting the code")
                    .with_hint(
                        "Start the sign-in again; if it repeats, reset anisette (`russet account anisette reset`).",
                    ))
            }
        }
    }

    async fn finish(
        &self,
        accounts: &tokio::sync::RwLock<AccountStore>,
        gsa: &Arc<GsaClient>,
        s: &GsSession,
        apple_id: &str,
        label: Option<String>,
        anisette: AnisetteSetting,
    ) -> Result<Account> {
        let token = gsa.xcode_token(s).await?;
        let qh = Qh65b2::new(self.ds2_base.clone(), Arc::new(Ds2Session::new(gsa.clone(), token.clone())))?;
        let teams = qh.list_teams().await?;
        let team = teams.iter().find(|t| t.tier() == TeamTier::Paid).or(teams.first());
        let entry = AppleIdEntry {
            dsid: s.dsid.clone(),
            apple_id: apple_id.trim().to_owned(),
            label: label.unwrap_or_else(|| format!("Apple ID ({})", masked_apple_id(apple_id.trim()))),
            team_id: team.map(|t| t.team_id.clone()),
            team_tier: team.map(|t| t.tier().as_str().to_owned()),
            anisette,
        };
        let stored = StoredSession::new(s, &token);
        self.secrets.put(&entry.secret_id(), "Apple ID session", SecretBytes::new(stored.to_bytes().to_vec())).await?;
        let account = accounts.write().await.upsert_apple_id(&entry)?;
        tracing::info!(tier = entry.team_tier.as_deref().unwrap_or("none"), "apple id signed in");
        Ok(account)
    }

    /// The developer portal of an Apple ID account's team, over developerservices2 (refreshing the Xcode token
    /// when it is about to expire).
    pub async fn portal(&self, entry: &AppleIdEntry, team: Option<&str>) -> Result<Arc<dyn DevPortal>> {
        let team = team.map(str::to_owned).or_else(|| entry.team_id.clone()).ok_or_else(|| {
            Error::new(codes::account::NO_ACCOUNT, "this Apple ID has no developer team yet")
                .with_hint("Sign in once at developer.apple.com to accept the agreement, then sign in to Russet again.")
        })?;
        let bytes = self.secrets.get(&entry.secret_id()).await?.ok_or_else(|| {
            Error::new(codes::gsa::SESSION_EXPIRED, "the Apple ID session is missing from the secret store")
                .with_hint("Sign in again (the Apple ID page of the IDE).")
        })?;
        let mut stored = StoredSession::from_bytes(bytes.expose())?;
        let gsa = self.client(&entry.anisette).await?;
        let mut token = stored.token()?;
        if !token.fresh(time::OffsetDateTime::now_utc(), time::Duration::minutes(5)) {
            token = gsa.xcode_token(&stored.session()?).await?;
            stored.set_token(&token);
            self.secrets
                .put(&entry.secret_id(), "Apple ID session", SecretBytes::new(stored.to_bytes().to_vec()))
                .await?;
        }
        let auth = Arc::new(Ds2Auth::new(self.ds2_base.clone(), team.clone(), Arc::new(Ds2Session::new(gsa, token))));
        let client = russet_asc::AscClient::new(auth.clone())?;
        let mut_client = russet_asc::AscClient::new(auth)?;
        Ok(Arc::new(AscPortal::new(client, mut_client, team, false)))
    }

    /// `account.anisette.status` (no helper is started, nothing is downloaded).
    pub async fn anisette_status(&self) -> AnisetteStatus {
        let set = libs::Abi::host().and_then(|abi| libs::installed(&self.dirs.data, abi));
        AnisetteStatus {
            libraries_present: set.is_some(),
            apk_version: set.as_ref().map(|s| s.apk_version.clone()),
            pinned: set.as_ref().map(|s| s.pinned),
            provisioned: LocalAdi::provisioned_offline(&*self.secrets).await,
            local_supported: russet_anisette::local_supported(),
        }
    }

    /// `account.anisette.fetchLibraries`.
    pub async fn fetch_libraries(&self, consent: bool) -> Result<Fetched> {
        let consent = Consent::given(consent)?;
        let fetched = libs::fetch(&self.dirs.data, &self.endpoints.anisette_libs, consent).await?;
        *self.local.lock().await = None;
        Ok(fetched)
    }

    /// Forgets the local provider (after a reset or new libraries).
    pub async fn drop_local(&self) {
        *self.local.lock().await = None;
    }
}

fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// The developer portal of an Apple ID account, through the service registered for `config` (the daemon's).
pub(crate) async fn portal_for(
    config: &std::path::Path,
    entry: &AppleIdEntry,
    team: Option<&str>,
) -> Result<Arc<dyn DevPortal>> {
    let svc = AppleIds::lookup(config).ok_or_else(|| {
        Error::new(codes::account::NO_ACCOUNT, "Apple ID accounts reach developer services through the daemon")
            .with_hint("Run the command through russetd (the IDE, or the CLI without --in-process).")
    })?;
    svc.portal(entry, team).await
}
