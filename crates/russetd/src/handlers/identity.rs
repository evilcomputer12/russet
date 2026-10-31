// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `identity` handlers (§8.4). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! `identity.list` reads the public parts of the identity store; `importP12`/`exportP12` are W12's and answer
//! `core.notImplemented` until then.

use russet_account::{AccountService, CertKind, SigningIdentity};
use russet_core::Result;
use russet_ipc::Router;
use russet_protocol::identity as p;

use super::account::{accounts_of, redacted};
use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::IdentityList, _, _>(|cx: DaemonCtx, _rq, params: p::IdentityListParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), list(&svc, params).await).await
    });
    r.request::<p::IdentityImportP12, _, _>(not_implemented::<p::IdentityImportP12>);
    r.request::<p::IdentityExportP12, _, _>(not_implemented::<p::IdentityExportP12>);
}

/// `identity.list`.
pub async fn list(svc: &AccountService, params: p::IdentityListParams) -> Result<p::IdentityListResult> {
    let identities = svc.identities().list(params.team.as_deref())?;
    Ok(p::IdentityListResult { identities: identities.iter().map(identity_dto).collect() })
}

/// The wire form of an identity: named by its certificate serial, the team masked, and no certificate subject
/// (it names a person).
pub fn identity_dto(i: &SigningIdentity) -> p::SigningIdentity {
    p::SigningIdentity {
        id: i.id.wire(),
        team_id: russet_sign::mask_team(&i.team_id),
        kind: match i.kind {
            CertKind::Development => p::CertificateKind::Development,
            CertKind::Distribution => p::CertificateKind::Distribution,
        },
        serial: i.serial.clone(),
        label: i.label.clone(),
        not_after: russet_account::rfc3339(i.not_after),
        expired: i.is_expired(),
        sha1: i.sha1.clone(),
        imported_from: i.imported_from.clone(),
    }
}
