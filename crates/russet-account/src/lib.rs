// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Accounts, identities, the developer portal and provisioning (ARCHITECTURE §5.3 `russet-account`; SIGN
//! §5–§6). [`AccountService`] is what `DaemonCtx.accounts` names [D 86].
//!
//! Wave 1 (W6, SA-06 subset): `accounts.toml`, `account.addAscKey`, the one-time xtool import [D 50], the
//! identity store, the GET-only [`AscPortal`], [`Provisioner::plan`] (GET-only, with profile matching that
//! reuses existing profiles such as xtool's `XTL-…` one) and [`Provisioner::apply`] for zero-mutation plans;
//! mutations are refused without an approval and applied by W12. W22 owns [`free`].

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, codes};
use russet_core::net::Endpoints;
use russet_core::time::Clock;
use russet_secrets::SecretStore;
use russet_sign::Distribution;

mod doctor;
pub mod free;
mod fsutil;
mod identity;
mod portal;
mod profiles;
mod project;
mod provision;
mod signing;
mod store;
mod xtool;

pub use doctor::{AccountConfigured, IdentityUsable, XtoolFilesPrivate};
pub use fsutil::{parse_rfc3339, rfc3339};
pub use identity::{CertKind, IdentityId, IdentityStore, LoadedIdentity, PendingKey, SigningIdentity};
pub use portal::{AscPortal, BundleIdRecord, CertInfo, DevPortal, DeviceRecord, PortalFlavor, ProfileRecord};
pub use profiles::{CachedProfile, ProfileCache, ProfileQuery};
pub use project::{BundleIdPolicySpec, ExtensionSpec, ProjectSigning};
pub use provision::{
    ApprovalRef, BundleProvision, BundleSpec, Mutation, PlanId, PlanWarning, PlannedProfile, ProvisionOutcome,
    ProvisionPlan, ProvisionRequest, Provisioner, redact_udid,
};
pub use signing::SignAppRequest;
pub use store::{Account, AccountId, AccountKind, AccountStore, AddAscKey, XtoolImport};

/// How long a plan stays applicable (ARCHITECTURE §11.3: pending approvals live 10 minutes).
pub const PLAN_TTL: Duration = Duration::from_secs(600);

/// How long an expired plan is still recognized (answered `provision.planStale`, not `core.notFound`).
pub const PLAN_MEMORY: Duration = Duration::from_secs(6 * 600);

/// The system clock (W2's `russet_core::time::SystemClock` is the shared one once it lands on main).
#[derive(Debug, Clone, Copy, Default)]
struct WallClock;

impl Clock for WallClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
    fn instant(&self) -> Instant {
        Instant::now()
    }
}

/// One probe of `account.verify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    Ok,
    /// The key's role cannot read this resource.
    Forbidden,
    /// The key was rejected.
    Unauthorized,
    /// Anything else, with its `russetCode`.
    Error(String),
}

impl ProbeResult {
    /// `ok`, `forbidden`, `unauthorized` or `error`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Forbidden => "forbidden",
            Self::Unauthorized => "unauthorized",
            Self::Error(_) => "error",
        }
    }

    fn of<T>(r: &Result<T>) -> Self {
        match r {
            Ok(_) => Self::Ok,
            Err(e) if e.code() == codes::account::ASC_UNAUTHORIZED => Self::Unauthorized,
            Err(e) if e.code() == codes::account::ASC_FORBIDDEN || e.code() == codes::account::INSUFFICIENT_ROLE => {
                Self::Forbidden
            }
            Err(e) => Self::Error(e.code().as_str().to_owned()),
        }
    }
}

/// What `account.verify` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountVerification {
    pub account_id: AccountId,
    /// The key is accepted and reads what its kind is for: certificates for a team key; builds for an individual
    /// key, which cannot use the provisioning endpoints (SIGN §5.2).
    pub ok: bool,
    /// The recorded team, else the one these reads discovered (`account.verify` writes nothing).
    pub team_id: Option<String>,
    pub certificates: ProbeResult,
    pub profiles: ProbeResult,
    pub builds: ProbeResult,
}

/// AccountStore, IdentityStore, the profile cache and the plans awaiting `provision.apply` (`DaemonCtx.accounts`).
pub struct AccountService {
    dirs: RussetDirs,
    endpoints: Endpoints,
    accounts: tokio::sync::RwLock<AccountStore>,
    identities: IdentityStore,
    profiles: ProfileCache,
    plans: Mutex<BTreeMap<String, (ProvisionPlan, Instant)>>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for AccountService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountService").field("config", &self.dirs.config).finish_non_exhaustive()
    }
}

impl AccountService {
    pub fn new(dirs: &RussetDirs, secrets: Arc<dyn SecretStore>, endpoints: Endpoints) -> Result<Self> {
        Self::with_xtool_dir(dirs, secrets, endpoints, None)
    }

    /// Like [`Self::new`], reading xtool's store from `xtool_dir` instead of `~/.config/xtool/data` (tests).
    pub fn with_xtool_dir(
        dirs: &RussetDirs,
        secrets: Arc<dyn SecretStore>,
        endpoints: Endpoints,
        xtool_dir: Option<PathBuf>,
    ) -> Result<Self> {
        let mut store = AccountStore::load(dirs, secrets.clone())?.with_asc_base(endpoints.asc.clone());
        if let Some(d) = xtool_dir {
            store = store.with_xtool_dir(d);
        }
        Ok(Self {
            dirs: dirs.clone(),
            identities: IdentityStore::new(&dirs.data, secrets),
            profiles: ProfileCache::new(&dirs.data),
            accounts: tokio::sync::RwLock::new(store),
            endpoints,
            plans: Mutex::new(BTreeMap::new()),
            clock: Arc::new(WallClock),
        })
    }

    /// The clock plans age by (tests substitute a manual one).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Every team ID the store knows (the accounts' and the identities'): what the daemon masks in anything it
    /// sends (AGENTS.md §8).
    pub async fn known_teams(&self) -> Vec<String> {
        let mut teams: Vec<String> = self.accounts.read().await.list().into_iter().filter_map(|a| a.team_id).collect();
        teams.extend(self.identities.list(None).unwrap_or_default().into_iter().map(|i| i.team_id));
        teams.sort();
        teams.dedup();
        teams
    }

    pub fn accounts(&self) -> &tokio::sync::RwLock<AccountStore> {
        &self.accounts
    }

    pub fn identities(&self) -> &IdentityStore {
        &self.identities
    }

    pub fn profiles(&self) -> &ProfileCache {
        &self.profiles
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// A provisioner for an account (the default one when `None`) and team (the account's when `None`).
    pub async fn provisioner(&self, account: Option<&AccountId>, team: Option<&str>) -> Result<Provisioner> {
        let store = self.accounts.read().await;
        let acc = store.resolve(account)?;
        let portal = store.portal(&acc.id, team).await?;
        Ok(Provisioner::new(portal, self.identities.clone(), self.profiles.clone(), acc.id))
    }

    /// `account.verify`: GET-scoped probes of certificates, profiles and builds, and the team (discovered when
    /// none is recorded). Read-only: it writes nothing, `accounts.toml` included (re-adding the key records a
    /// team).
    pub async fn verify(&self, account: Option<&AccountId>) -> Result<AccountVerification> {
        let (acc, key, client) = {
            let store = self.accounts.read().await;
            let acc = store.resolve(account)?;
            let key = store.asc_key(&acc).await?;
            let client = store.client(&key)?;
            (acc, key, client)
        };
        let certificates = ProbeResult::of(&client.certificates().probe(1).await);
        let profiles = ProbeResult::of(&client.get_json::<serde_json::Value>("/v1/profiles?limit=1").await);
        let builds = ProbeResult::of(&client.builds().recent(1).await);
        let mut team_id = acc.team_id.clone();
        if team_id.is_none() && certificates == ProbeResult::Ok {
            team_id = store::discover_team(&client, &key, &self.identities).await.ok().flatten();
        }
        let ok = if key.is_individual() { builds == ProbeResult::Ok } else { certificates == ProbeResult::Ok };
        Ok(AccountVerification { ok, account_id: acc.id, team_id, certificates, profiles, builds })
    }

    /// Plans provisioning for a project (`provision.plan {project}`): reads its signing inputs, applies the
    /// bundle-ID policy and plans with the account's portal. The plan is kept for `provision.apply`.
    pub async fn plan_project(
        &self,
        project: &Path,
        distribution: Distribution,
        devices: Vec<String>,
        account: Option<&AccountId>,
    ) -> Result<ProvisionPlan> {
        let signing = ProjectSigning::read(project)?;
        let account_id = account.cloned().or_else(|| signing.account.clone().map(AccountId));
        let (acc, team) = {
            let store = self.accounts.read().await;
            let acc = store.resolve(account_id.as_ref())?;
            let team = signing.team.clone().or_else(|| acc.team_id.clone());
            (acc, team)
        };
        let request = signing.request(&acc, team.as_deref(), distribution, devices)?;
        let provisioner = self.provisioner(Some(&acc.id), team.as_deref()).await?;
        let plan = provisioner.plan(&request).await?;
        self.remember_plan(&plan);
        Ok(plan)
    }

    /// Keeps a plan for [`Self::apply_plan`]: applicable for [`PLAN_TTL`], recognized as stale for
    /// [`PLAN_MEMORY`].
    pub fn remember_plan(&self, plan: &ProvisionPlan) {
        let now = self.clock.instant();
        if let Ok(mut plans) = self.plans.lock() {
            plans.retain(|_, (_, at)| now.saturating_duration_since(*at) < PLAN_MEMORY);
            plans.insert(plan.plan_id.0.clone(), (plan.clone(), now));
        }
    }

    /// A remembered plan: `provision.planStale` once it is older than [`PLAN_TTL`], `core.notFound` for an id this
    /// daemon never planned (or forgot after [`PLAN_MEMORY`]).
    pub fn plan(&self, plan_id: &str) -> Result<ProvisionPlan> {
        let plans = self.plans.lock().map_err(|_| Error::new(codes::core::INTERNAL, "the plan table is poisoned"))?;
        let again = "Plan again with `russet provision --dry-run` (or `provision.plan`), then apply the new plan.";
        match plans.get(plan_id) {
            Some((p, at)) if self.clock.instant().saturating_duration_since(*at) < PLAN_TTL => Ok(p.clone()),
            Some(_) => Err(Error::new(
                codes::provision::PLAN_STALE,
                format!("plan {plan_id} has expired (plans can be applied for 10 minutes)"),
            )
            .with_hint(again)
            .with_details(serde_json::json!({"planId": plan_id, "reason": "expired"}))),
            None => Err(Error::new(codes::core::NOT_FOUND, format!("no plan {plan_id}")).with_hint(again)),
        }
    }

    /// `asc.get`: an allowlisted App Store Connect GET with a request-scoped token (SIGN §11). Returns the HTTP
    /// status and the JSON body; the allowlist and path normalization are checked by the client.
    pub async fn asc_get(&self, account: Option<&AccountId>, path_and_query: &str) -> Result<(u16, serde_json::Value)> {
        let client = {
            let store = self.accounts.read().await;
            let acc = store.resolve(account)?;
            let key = store.asc_key(&acc).await?;
            store.client(&key)?
        };
        client.raw_get(path_and_query).await
    }

    /// `provision.apply`.
    pub async fn apply_plan(&self, plan_id: &str, approval: &ApprovalRef) -> Result<ProvisionOutcome> {
        let plan = self.plan(plan_id)?;
        let provisioner = self.provisioner(Some(&plan.account_id), Some(&plan.team_id)).await?;
        provisioner.apply(&plan, approval).await
    }
}

/// Account doctor checks, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    vec![
        Arc::new(AccountConfigured::default()),
        Arc::new(IdentityUsable::default()),
        Arc::new(XtoolFilesPrivate::default()),
    ]
}
