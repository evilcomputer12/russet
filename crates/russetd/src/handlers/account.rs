// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `account` handlers (§8.4). Owner: W6 (Wave 1), W12 (Wave 2), W22 (appleId and anisette rows, Wave 3).
//!
//! Wave 1: `account.list`, `account.addAscKey`, `account.importXtool` and `account.verify`. The two imports are
//! `secrets`-class local-file imports; the permission engine lets a non-interactive CLI run them after a plain
//! confirmation that `--yes` pre-confirms (ARCHITECTURE §7.5 [D 85]). The other rows answer
//! `core.notImplemented` until W12 and W22.
//!
//! **No team ID on the wire** (AGENTS.md §8): the DTOs mask team IDs (`*******345`) and name identities by their
//! certificate serial, and every W6 handler passes its error through [`redacted`], which masks each team ID the
//! account service knows in the message, hint, details and cause chain.

use std::sync::Arc;

use russet_account::{Account, AccountId, AccountKind, AccountService, AccountVerification, AddAscKey, ProbeResult};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_ipc::Router;
use russet_protocol::account as p;

use super::identity::identity_dto;
use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::AccountList, _, _>(|cx: DaemonCtx, _rq, _p: p::AccountListParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), list(&svc).await).await
    });
    r.request::<p::AccountAddAscKey, _, _>(|cx: DaemonCtx, _rq, params: p::AccountAddAscKeyParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), add_asc_key(&svc, params).await).await
    });
    r.request::<p::AccountImportXtool, _, _>(|cx: DaemonCtx, _rq, params: p::AccountImportXtoolParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), import_xtool(&svc, params).await).await
    });
    r.request::<p::AccountVerify, _, _>(|cx: DaemonCtx, _rq, params: p::AccountVerifyParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), verify(&svc, params).await).await
    });
    r.request::<p::AccountRemove, _, _>(|cx: DaemonCtx, _rq, params: p::AccountRemoveParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), remove(&svc, params).await).await
    });
    r.request::<p::AccountSetDefault, _, _>(|cx: DaemonCtx, _rq, params: p::AccountSetDefaultParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), set_default(&svc, params).await).await
    });
    r.request::<p::AccountAppleIdBegin, _, _>(not_implemented::<p::AccountAppleIdBegin>);
    r.request::<p::AccountAppleIdRequestCode, _, _>(not_implemented::<p::AccountAppleIdRequestCode>);
    r.request::<p::AccountAppleIdSubmitCode, _, _>(not_implemented::<p::AccountAppleIdSubmitCode>);
    r.request::<p::AccountAnisetteStatus, _, _>(not_implemented::<p::AccountAnisetteStatus>);
    r.request::<p::AccountAnisetteFetchLibraries, _, _>(not_implemented::<p::AccountAnisetteFetchLibraries>);
}

/// The account service of the daemon context.
pub fn accounts_of(cx: &DaemonCtx) -> Result<Arc<AccountService>> {
    cx.accounts.get()
}

/// Reads a service from its `DaemonCtx` slot. The F0 context holds the services as plain `Arc`s; the daemon
/// runtime (W2) turns the slots into lazily started `Service`s, whose inherent `get()` takes precedence over this
/// trait, so [`accounts_of`] compiles against both. Delete it once W2 is on main.
#[allow(dead_code)] // unused once the slots are W2's `Service`s
trait Slot<T: ?Sized> {
    fn get(&self) -> Result<Arc<T>>;
}

impl<T: ?Sized> Slot<T> for Arc<T> {
    fn get(&self) -> Result<Arc<T>> {
        Ok(Arc::clone(self))
    }
}

/// `r` with every team ID the account service knows masked in its error (the wire boundary of the W6 handlers;
/// errors are assembled from paths, profiles and third-party messages that can name the team).
pub async fn redacted<T>(svc: Option<&AccountService>, r: Result<T>) -> Result<T> {
    match r {
        Ok(v) => Ok(v),
        Err(e) => {
            let teams = match svc {
                Some(s) => s.known_teams().await,
                None => Vec::new(),
            };
            Err(russet_sign::redact_error(e, &teams))
        }
    }
}

/// Like [`redacted`] for handlers that need no account: the account service is reached only when there is an
/// error to redact (and skipped when it cannot start).
pub async fn redacted_cx<T>(cx: &DaemonCtx, r: Result<T>) -> Result<T> {
    match r {
        Ok(v) => Ok(v),
        Err(e) => redacted(accounts_of(cx).ok().as_deref(), Err(e)).await,
    }
}

/// `account.list`.
pub async fn list(svc: &AccountService) -> Result<p::AccountListResult> {
    let store = svc.accounts().read().await;
    let accounts: Vec<p::Account> = store.list().iter().map(account_dto).collect();
    let default = accounts.iter().find(|a| a.is_default).map(|a| a.id.clone());
    Ok(p::AccountListResult { accounts, default })
}

/// `account.addAscKey`.
pub async fn add_asc_key(svc: &AccountService, params: p::AccountAddAscKeyParams) -> Result<p::Account> {
    let p8_path = absolute(&params.p8_path, "p8Path")?;
    let account = svc
        .accounts()
        .write()
        .await
        .add_asc_key(AddAscKey { key_id: params.key_id, issuer_id: params.issuer_id, p8_path, label: params.label })
        .await?;
    Ok(account_dto(&account))
}

/// `account.importXtool`.
pub async fn import_xtool(
    svc: &AccountService,
    params: p::AccountImportXtoolParams,
) -> Result<p::AccountImportXtoolResult> {
    let dry_run = params.dry_run.unwrap_or(false);
    let imp = svc.accounts().write().await.import_xtool(dry_run).await?;
    Ok(p::AccountImportXtoolResult {
        accounts: imp.accounts.iter().map(account_dto).collect(),
        identities: imp.identities.iter().map(identity_dto).collect(),
        warnings: imp.warnings,
        dry_run: imp.dry_run,
    })
}

/// `account.remove`.
pub async fn remove(svc: &AccountService, params: p::AccountRemoveParams) -> Result<p::AccountRemoveResult> {
    svc.accounts().write().await.remove(&AccountId(params.account_id), params.wipe_secrets.unwrap_or(false)).await?;
    Ok(p::AccountRemoveResult {})
}

/// `account.setDefault`.
pub async fn set_default(
    svc: &AccountService,
    params: p::AccountSetDefaultParams,
) -> Result<p::AccountSetDefaultResult> {
    svc.accounts().write().await.set_default(&AccountId(params.account_id))?;
    Ok(p::AccountSetDefaultResult {})
}

/// `account.verify`.
pub async fn verify(svc: &AccountService, params: p::AccountVerifyParams) -> Result<p::AccountVerifyResult> {
    let id = params.account_id.map(AccountId);
    let v = svc.verify(id.as_ref()).await?;
    Ok(verification_dto(&v))
}

/// The wire form of an account (team masked).
pub fn account_dto(a: &Account) -> p::Account {
    p::Account {
        id: a.id.0.clone(),
        kind: match a.kind {
            AccountKind::AscTeamKey => p::AccountKind::AscTeamKey,
            AccountKind::AscIndividualKey => p::AccountKind::AscIndividualKey,
            AccountKind::AppleId => p::AccountKind::AppleId,
        },
        label: a.label.clone(),
        key_id: a.key_id.clone(),
        team_id: a.team_id.as_deref().map(russet_sign::mask_team),
        imported_from: a.imported_from.clone(),
        is_default: a.is_default,
    }
}

/// The wire form of `account.verify`.
pub fn verification_dto(v: &AccountVerification) -> p::AccountVerifyResult {
    let probe = |r: &ProbeResult| match r {
        ProbeResult::Ok => p::ProbeOutcome::Ok,
        ProbeResult::Forbidden => p::ProbeOutcome::Forbidden,
        ProbeResult::Unauthorized => p::ProbeOutcome::Unauthorized,
        ProbeResult::Error(_) => p::ProbeOutcome::Error,
    };
    p::AccountVerifyResult {
        ok: v.ok,
        account_id: v.account_id.0.clone(),
        team_id: v.team_id.as_deref().map(russet_sign::mask_team),
        role_probe: p::RoleProbe {
            certificates: probe(&v.certificates),
            profiles: probe(&v.profiles),
            builds: probe(&v.builds),
        },
    }
}

/// Paths on the wire are absolute (ARCHITECTURE §7.3).
pub(crate) fn absolute(path: &str, field: &str) -> Result<std::path::PathBuf> {
    let p = std::path::PathBuf::from(path);
    if p.is_absolute() {
        Ok(p)
    } else {
        Err(Error::new(codes::core::INVALID_ARGUMENT, format!("{field} must be an absolute path, got `{path}`"))
            .with_hint("Clients send absolute paths (the CLI resolves them against its working directory)."))
    }
}
