// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Accounts, identities, the developer portal and provisioning (ARCHITECTURE §5.3 `russet-account`; SIGN
//! §5–§6). [`AccountService`] is what `DaemonCtx.accounts` names [D 86].
//!
//! F0 stub (W1); W6 fills it (SA-06 subset, Wave 1), W12 (Wave 2); W22 owns [`free`].

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::ids::ApprovalId;
use russet_core::net::Endpoints;
use russet_secrets::{SecretBytes, SecretStore};
use russet_sign::{CodeSigningIdentity, KeyInfoSigner};

pub mod free;

/// An account id: `asc:<KEYID>` or `appleid:<DSID>` (ARCHITECTURE §14.4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(pub String);

/// One account from `accounts.toml`. W6 defines the remaining fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: AccountId,
}

/// `account.addAscKey` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddAscKey {
    pub key_id: String,
    pub issuer_id: Option<String>,
    pub p8_path: PathBuf,
    pub label: Option<String>,
}

/// What the one-time xtool import found or did [D 50]. W6 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XtoolImport {
    _f0: (),
}

/// The developer portal of one team (ASC team key, or the free Apple ID's DS2 session).
#[async_trait]
pub trait DevPortal: Send + Sync {}

/// `accounts.toml` plus the sealed secrets.
#[derive(Debug)]
pub struct AccountStore {
    _f0: (),
}

impl AccountStore {
    pub fn load(dirs: &RussetDirs, secrets: Arc<dyn SecretStore>) -> Result<Self> {
        todo!("W6 (SA-06)")
    }
    pub fn list(&self) -> Vec<Account> {
        todo!("W6 (SA-06)")
    }
    pub async fn add_asc_key(&mut self, r: AddAscKey) -> Result<Account> {
        todo!("W6 (SA-06)")
    }
    pub async fn import_xtool(&mut self, dry_run: bool) -> Result<XtoolImport> {
        todo!("W6 (SA-06)")
    }
    pub async fn portal(&self, id: &AccountId, team: Option<&str>) -> Result<Arc<dyn DevPortal>> {
        todo!("W6 (SA-06)")
    }
}

/// A signing identity id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IdentityId(pub String);

/// A signing identity's public part (`data/identities/<team>/<serial>.{json,cer}`). W6 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningIdentity {
    pub id: IdentityId,
}

/// Certificate kind for a new CSR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertKind {
    Development,
    Distribution,
}

/// A private key generated for a CSR, before the certificate exists. W6 defines the fields.
#[derive(Debug)]
pub struct PendingKey {
    _f0: (),
}

/// A loaded identity: certificate plus private key; implements [`CodeSigningIdentity`].
#[derive(Debug)]
pub struct LoadedIdentity {
    _f0: (),
}

impl CodeSigningIdentity for LoadedIdentity {
    fn certificate_der(&self) -> &[u8] {
        todo!("W6 (SA-06)")
    }
    fn team_id(&self) -> &str {
        todo!("W6 (SA-06)")
    }
    fn signer(&self) -> &dyn KeyInfoSigner {
        todo!("W6 (SA-06)")
    }
}

/// Signing identities (public parts on disk, private keys sealed).
#[derive(Debug)]
pub struct IdentityStore {
    _f0: (),
}

impl IdentityStore {
    pub fn list(&self, team: Option<&str>) -> Result<Vec<SigningIdentity>> {
        todo!("W6 (SA-06)")
    }
    pub fn load(&self, id: &IdentityId) -> Result<LoadedIdentity> {
        todo!("W6 (SA-06)")
    }
    pub fn import_p12(&self, p12: &[u8], pw: &SecretBytes) -> Result<SigningIdentity> {
        todo!("W6 (SA-06)")
    }
    /// Returns the pending key and the PEM CSR.
    pub fn new_csr(&self, team: &str, k: CertKind) -> Result<(PendingKey, String)> {
        todo!("W12")
    }
}

/// A provisioning plan id (`pln_…`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlanId(pub String);

/// What to provision for (`provision.plan` params). W6 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionRequest {
    _f0: (),
}

/// One Apple-side mutation of a plan. W6 and W12 define the variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mutation {
    _f0: (),
}

/// A plan warning. W6 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanWarning {
    pub message: String,
}

/// A GET-only provisioning plan; `digest` binds approvals to it (ARCHITECTURE §11.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionPlan {
    pub plan_id: PlanId,
    pub digest: [u8; 32],
    pub team_id: String,
    pub mutations: Vec<Mutation>,
    pub warnings: Vec<PlanWarning>,
}

/// The approval that authorizes a plan's mutations (none for a zero-mutation plan).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRef {
    pub approval_id: Option<ApprovalId>,
}

/// What `provision.apply` did. W6 and W12 define the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionOutcome {
    _f0: (),
}

/// Plans and applies provisioning for one team.
#[derive(Debug)]
pub struct Provisioner {
    _f0: (),
}

impl Provisioner {
    /// GET-only.
    pub async fn plan(&self, r: &ProvisionRequest) -> Result<ProvisionPlan> {
        todo!("W6 (SA-06)")
    }
    pub async fn apply(&self, plan: &ProvisionPlan, approval: &ApprovalRef) -> Result<ProvisionOutcome> {
        todo!("W6 (zero-mutation) / W12")
    }
}

/// AccountStore, IdentityStore and the per-account TokenFactory and DevPortal caches (`DaemonCtx.accounts`).
#[derive(Debug)]
pub struct AccountService {
    _f0: (),
}

impl AccountService {
    pub fn new(dirs: &RussetDirs, secrets: Arc<dyn SecretStore>, endpoints: Endpoints) -> Result<Self> {
        todo!("W6 (SA-06)")
    }
    pub fn accounts(&self) -> &tokio::sync::RwLock<AccountStore> {
        todo!("W6 (SA-06)")
    }
    pub fn identities(&self) -> &IdentityStore {
        todo!("W6 (SA-06)")
    }
    pub async fn provisioner(&self, account: Option<&AccountId>, team: Option<&str>) -> Result<Provisioner> {
        todo!("W6 (SA-06)")
    }
}

/// Account doctor checks, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
