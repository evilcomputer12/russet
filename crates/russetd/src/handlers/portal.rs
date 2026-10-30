// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `portal` handlers (§8.4). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! The list rows read the account's developer portal with GET-scoped tokens. UDIDs, device names and certificate
//! display names (which name a person) are masked or omitted unless an `ide` connection asks with `reveal: true`
//! (ARCHITECTURE §8.4, §11.6; [`reveal_allowed`]). `devices.register` and `certificates.revoke` are W12's.

use russet_account::{AccountId, AccountService, CertInfo, DevPortal, DeviceRecord};
use russet_core::Result;
use russet_core::rpc::ClientKind;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::portal as p;

use super::account::{accounts_of, redacted};
use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::PortalCertificatesList, _, _>(
        |cx: DaemonCtx, rq: RequestCtx, params: p::PortalListParams| async move {
            let reveal = reveal_allowed(&params, rq.peer.connection().kind);
            let svc = accounts_of(&cx)?;
            redacted(Some(&svc), certificates(&svc, params, reveal).await).await
        },
    );
    r.request::<p::PortalDevicesList, _, _>(|cx: DaemonCtx, rq: RequestCtx, params: p::PortalListParams| async move {
        let reveal = reveal_allowed(&params, rq.peer.connection().kind);
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), devices(&svc, params, reveal).await).await
    });
    r.request::<p::PortalBundleIdsList, _, _>(|cx: DaemonCtx, _rq, params: p::PortalListParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), bundle_ids(&svc, params).await).await
    });
    r.request::<p::PortalProfilesList, _, _>(|cx: DaemonCtx, _rq, params: p::PortalListParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), profiles(&svc, params).await).await
    });
    r.request::<p::PortalDevicesRegister, _, _>(not_implemented::<p::PortalDevicesRegister>);
    r.request::<p::PortalCertificatesRevoke, _, _>(
        |cx: DaemonCtx, _rq, params: p::PortalCertificatesRevokeParams| async move {
            let svc = accounts_of(&cx)?;
            redacted(Some(&svc), revoke_certificate(&svc, params).await).await
        },
    );
}

/// `portal.certificates.revoke`: revokes a certificate by its portal id (§6.7). The typed confirmation is
/// enforced by the permission engine (`Confirm::Typed`, cli/ide only) before the handler runs.
pub async fn revoke_certificate(
    svc: &AccountService,
    params: p::PortalCertificatesRevokeParams,
) -> Result<p::PortalCertificatesRevokeResult> {
    let portal = portal(svc, params.account_id).await?;
    portal.revoke_certificate(&params.cert_id, &russet_account::ApprovalRef::default()).await?;
    Ok(p::PortalCertificatesRevokeResult {})
}

/// Whether a listing may show UDIDs, device names and certificate display names: only an `ide` connection that
/// asked with `reveal: true` (the IDE shows them to the user it runs for; agents and the CLI never get them).
pub fn reveal_allowed(params: &p::PortalListParams, kind: ClientKind) -> bool {
    params.reveal == Some(true) && kind == ClientKind::Ide
}

async fn portal(svc: &AccountService, account: Option<String>) -> Result<std::sync::Arc<dyn DevPortal>> {
    let store = svc.accounts().read().await;
    let acc = store.resolve(account.map(AccountId).as_ref())?;
    store.portal(&acc.id, None).await
}

/// `portal.certificates.list`; `reveal` ([`reveal_allowed`]) adds the display names.
pub async fn certificates(
    svc: &AccountService,
    params: p::PortalListParams,
    reveal: bool,
) -> Result<p::PortalCertificatesListResult> {
    let portal = portal(svc, params.account_id).await?;
    let local: Vec<String> = svc.identities().list(None)?.into_iter().map(|i| i.serial).collect();
    let certs = portal.list_certificates(params.filter.as_deref()).await?;
    Ok(certificates_dto(certs, &local, reveal))
}

/// The wire form of portal certificates: the display name (it can name a person) only when revealed.
pub fn certificates_dto(
    certs: Vec<CertInfo>,
    local_serials: &[String],
    reveal: bool,
) -> p::PortalCertificatesListResult {
    let certificates = certs
        .into_iter()
        .map(|c| p::PortalCertificate {
            local_identity: c.serial.as_ref().is_some_and(|s| local_serials.iter().any(|l| l.eq_ignore_ascii_case(s))),
            id: c.id,
            certificate_type: c.certificate_type,
            serial: c.serial,
            display_name: if reveal { c.display_name } else { None },
            expires: c.expiration,
        })
        .collect();
    p::PortalCertificatesListResult { certificates }
}

/// `portal.devices.list`; `reveal` ([`reveal_allowed`]) unmasks UDIDs and adds device names.
pub async fn devices(
    svc: &AccountService,
    params: p::PortalListParams,
    reveal: bool,
) -> Result<p::PortalDevicesListResult> {
    let portal = portal(svc, params.account_id).await?;
    Ok(devices_dto(portal.list_devices().await?, reveal))
}

/// The wire form of registered devices: UDIDs masked (`0000…5F60`) and no names, unless revealed.
pub fn devices_dto(records: Vec<DeviceRecord>, reveal: bool) -> p::PortalDevicesListResult {
    let devices = records
        .into_iter()
        .map(|d| p::PortalDevice {
            id: d.id,
            udid: if reveal { d.udid } else { russet_account::redact_udid(&d.udid) },
            name: if reveal { d.name } else { None },
            platform: d.platform,
            device_class: d.device_class,
            status: d.status,
        })
        .collect();
    p::PortalDevicesListResult { devices }
}

/// `portal.bundleIds.list` (`filter`: an identifier prefix).
pub async fn bundle_ids(svc: &AccountService, params: p::PortalListParams) -> Result<p::PortalBundleIdsListResult> {
    let portal = portal(svc, params.account_id).await?;
    let bundle_ids = portal
        .list_bundle_ids()
        .await?
        .into_iter()
        .filter(|b| params.filter.as_deref().is_none_or(|f| b.identifier.starts_with(f)))
        .map(|b| p::PortalBundleId {
            id: b.id,
            identifier: b.identifier,
            name: b.name,
            platform: b.platform,
            capabilities: b.capabilities,
        })
        .collect();
    Ok(p::PortalBundleIdsListResult { bundle_ids })
}

/// `portal.profiles.list` (`filter`: a profile type).
pub async fn profiles(svc: &AccountService, params: p::PortalListParams) -> Result<p::PortalProfilesListResult> {
    let portal = portal(svc, params.account_id).await?;
    let profiles = portal
        .list_profiles(params.filter.as_deref())
        .await?
        .into_iter()
        .map(|x| p::PortalProfile {
            id: x.id,
            name: x.name,
            profile_type: x.profile_type,
            state: x.state,
            uuid: x.uuid,
            expires: x.expiration,
        })
        .collect();
    Ok(p::PortalProfilesListResult { profiles })
}
