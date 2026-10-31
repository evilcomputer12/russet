// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `account` handlers (§8.4). Owner: W6 (Wave 1), W12 (Wave 2), W22 (appleId and anisette rows, Wave 3).
//!
//! Wave 1: `account.list`, `account.addAscKey`, `account.importXtool` and `account.verify`. The two imports are
//! `secrets`-class local-file imports; the permission engine lets a non-interactive CLI run them after a plain
//! confirmation that `--yes` pre-confirms (ARCHITECTURE §7.5 [D 85]).
//!
//! W22: `account.appleId.begin/requestCode/submitCode` sign in to Apple with Russet's own GrandSlam client
//! (`russet_account::free::AppleIds`): passwords and codes go from the params into zeroized buffers and are
//! never logged or echoed; the account comes back with its team masked and a label that masks the Apple ID.
//! `account.anisette.status` reads local state only; `account.anisette.fetchLibraries` downloads Apple's ADI
//! libraries by HTTP range after the client's consent screen.
//!
//! **No team ID on the wire** (AGENTS.md §8): the DTOs mask team IDs (`*******345`) and name identities by their
//! certificate serial, and every W6 handler passes its error through [`redacted`], which masks each team ID the
//! account service knows in the message, hint, details and cause chain.

use std::sync::Arc;

use russet_account::free::{AnisetteSetting, AppleIds, BeginOutcome};
use russet_account::{Account, AccountId, AccountKind, AccountService, AccountVerification, AddAscKey, ProbeResult};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_ipc::Router;
use russet_protocol::account as p;

use super::identity::identity_dto;
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
    r.request::<p::AccountAppleIdBegin, _, _>(|cx: DaemonCtx, _rq, params: p::AccountAppleIdBeginParams| async move {
        let svc = accounts_of(&cx)?;
        let ids = apple_ids(&cx, &svc)?;
        redacted(Some(&svc), apple_id_begin(&svc, &ids, params).await).await
    });
    r.request::<p::AccountAppleIdRequestCode, _, _>(
        |cx: DaemonCtx, _rq, params: p::AccountAppleIdRequestCodeParams| async move {
            let svc = accounts_of(&cx)?;
            let ids = apple_ids(&cx, &svc)?;
            redacted(Some(&svc), apple_id_request_code(&ids, params).await).await
        },
    );
    r.request::<p::AccountAppleIdSubmitCode, _, _>(
        |cx: DaemonCtx, _rq, params: p::AccountAppleIdSubmitCodeParams| async move {
            let svc = accounts_of(&cx)?;
            let ids = apple_ids(&cx, &svc)?;
            redacted(Some(&svc), apple_id_submit_code(&svc, &ids, params).await).await
        },
    );
    r.request::<p::AccountAnisetteStatus, _, _>(|cx: DaemonCtx, _rq, _p: p::AccountAnisetteStatusParams| async move {
        let svc = accounts_of(&cx)?;
        let ids = apple_ids(&cx, &svc)?;
        Ok(anisette_status(&ids).await)
    });
    r.request::<p::AccountAnisetteFetchLibraries, _, _>(
        |cx: DaemonCtx, rq: russet_ipc::RequestCtx, params: p::AccountAnisetteFetchLibrariesParams| async move {
            let svc = accounts_of(&cx)?;
            let ids = apple_ids(&cx, &svc)?;
            tokio::select! {
                r = anisette_fetch_libraries(&ids, params) => r,
                () = rq.cancel.cancelled() => Err(Error::cancelled()),
            }
        },
    );
}

/// The Apple ID service of this daemon's Russet home (W22).
pub fn apple_ids(cx: &DaemonCtx, svc: &AccountService) -> Result<Arc<AppleIds>> {
    Ok(AppleIds::shared(&cx.dirs, cx.secrets.get()?, svc.endpoints().clone()))
}

/// `account.appleId.begin`.
pub async fn apple_id_begin(
    svc: &AccountService,
    ids: &AppleIds,
    params: p::AccountAppleIdBeginParams,
) -> Result<p::AccountAppleIdBeginResult> {
    let anisette = match params.anisette {
        p::AnisetteChoice::Local(_) => AnisetteSetting::Local,
        p::AnisetteChoice::Remote { remote } => AnisetteSetting::Remote(remote),
    };
    let password = russet_secrets::SecretBytes::new(params.password.into_bytes());
    let outcome = ids.begin(svc.accounts(), &params.apple_id, password, anisette, params.label).await?;
    Ok(match outcome {
        BeginOutcome::Done { session_id, account } => p::AccountAppleIdBeginResult {
            session_id,
            step: p::AppleIdStep::Done,
            two_factor: None,
            account: Some(account_dto(&account)),
        },
        BeginOutcome::TwoFactor { session_id, info } => {
            let mut via = Vec::new();
            if info.trusted_device {
                via.push(p::TwoFactorVia::TrustedDevice);
            }
            via.push(p::TwoFactorVia::Sms);
            p::AccountAppleIdBeginResult {
                session_id,
                step: p::AppleIdStep::TwoFactor,
                two_factor: Some(p::TwoFactorInfo {
                    via,
                    code_sent_to: info.pushed.then_some(p::TwoFactorVia::TrustedDevice),
                    phone_numbers: info
                        .phone_numbers
                        .into_iter()
                        .map(|n| p::TrustedPhone { id: n.id, number: n.obfuscated })
                        .collect(),
                    code_length: 6,
                }),
                account: None,
            }
        }
    })
}

/// `account.appleId.requestCode`.
pub async fn apple_id_request_code(
    ids: &AppleIds,
    params: p::AccountAppleIdRequestCodeParams,
) -> Result<p::AccountAppleIdRequestCodeResult> {
    let via = match (params.via, params.phone_id) {
        (p::TwoFactorVia::TrustedDevice, _) => russet_gsa::TwoFactorVia::TrustedDevice,
        (p::TwoFactorVia::Sms, Some(phone_id)) => russet_gsa::TwoFactorVia::Sms { phone_id },
        (p::TwoFactorVia::Sms, None) => {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "an SMS code needs `phoneId`")
                .with_hint("Pass one of the `twoFactor.phoneNumbers[].id` of `account.appleId.begin`."));
        }
    };
    ids.request_code(&params.session_id, via).await?;
    Ok(p::AccountAppleIdRequestCodeResult {})
}

/// `account.appleId.submitCode`.
pub async fn apple_id_submit_code(
    svc: &AccountService,
    ids: &AppleIds,
    params: p::AccountAppleIdSubmitCodeParams,
) -> Result<p::Account> {
    let account = ids.submit_code(svc.accounts(), &params.session_id, &params.code).await?;
    Ok(account_dto(&account))
}

/// `account.anisette.status`: the local provider (remote servers are set per account).
pub async fn anisette_status(ids: &AppleIds) -> p::AccountAnisetteStatusResult {
    let s = ids.anisette_status().await;
    p::AccountAnisetteStatusResult {
        kind: p::AnisetteKind::Local,
        libraries_present: s.libraries_present,
        apk_version: s.apk_version,
        provisioned: s.provisioned,
        local_supported: s.local_supported,
        pinned: s.pinned,
    }
}

/// `account.anisette.fetchLibraries`.
pub async fn anisette_fetch_libraries(
    ids: &AppleIds,
    params: p::AccountAnisetteFetchLibrariesParams,
) -> Result<p::AccountAnisetteFetchLibrariesResult> {
    let f = ids.fetch_libraries(params.consent).await?;
    Ok(p::AccountAnisetteFetchLibrariesResult {
        apk_version: f.set.apk_version,
        bytes_downloaded: f.bytes_downloaded,
        pinned: f.set.pinned,
    })
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
