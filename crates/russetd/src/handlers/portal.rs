// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `portal` handlers (§8.4). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! The list rows read the account's developer portal with GET-scoped tokens. UDIDs and device names are masked
//! unless an `ide` connection asks with `reveal: true` (ARCHITECTURE §8.4, §11.6). `devices.register` and
//! `certificates.revoke` are W12's.

use russet_account::{AccountId, AccountService, DevPortal};
use russet_core::Result;
use russet_core::rpc::ClientKind;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::portal as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::PortalCertificatesList, _, _>(|cx: DaemonCtx, _rq, params: p::PortalListParams| async move {
        certificates(&cx.accounts, params).await
    });
    r.request::<p::PortalDevicesList, _, _>(|cx: DaemonCtx, rq: RequestCtx, params: p::PortalListParams| async move {
        // Only a reveal request needs the caller's kind.
        let reveal = params.reveal == Some(true) && rq.peer.connection().kind == ClientKind::Ide;
        devices(&cx.accounts, params, reveal).await
    });
    r.request::<p::PortalBundleIdsList, _, _>(|cx: DaemonCtx, _rq, params: p::PortalListParams| async move {
        bundle_ids(&cx.accounts, params).await
    });
    r.request::<p::PortalProfilesList, _, _>(|cx: DaemonCtx, _rq, params: p::PortalListParams| async move {
        profiles(&cx.accounts, params).await
    });
    r.request::<p::PortalDevicesRegister, _, _>(not_implemented::<p::PortalDevicesRegister>);
    r.request::<p::PortalCertificatesRevoke, _, _>(not_implemented::<p::PortalCertificatesRevoke>);
}

async fn portal(svc: &AccountService, account: Option<String>) -> Result<std::sync::Arc<dyn DevPortal>> {
    let store = svc.accounts().read().await;
    let acc = store.resolve(account.map(AccountId).as_ref())?;
    store.portal(&acc.id, None).await
}

/// `portal.certificates.list`.
pub async fn certificates(
    svc: &AccountService,
    params: p::PortalListParams,
) -> Result<p::PortalCertificatesListResult> {
    let portal = portal(svc, params.account_id).await?;
    let local: Vec<String> = svc.identities().list(None)?.into_iter().map(|i| i.serial).collect();
    let certificates = portal
        .list_certificates(params.filter.as_deref())
        .await?
        .into_iter()
        .map(|c| p::PortalCertificate {
            local_identity: c.serial.as_ref().is_some_and(|s| local.iter().any(|l| l.eq_ignore_ascii_case(s))),
            id: c.id,
            certificate_type: c.certificate_type,
            serial: c.serial,
            display_name: c.display_name,
            expires: c.expiration,
        })
        .collect();
    Ok(p::PortalCertificatesListResult { certificates })
}

/// `portal.devices.list`; `reveal` only for an `ide` connection that asked for it.
pub async fn devices(
    svc: &AccountService,
    params: p::PortalListParams,
    reveal: bool,
) -> Result<p::PortalDevicesListResult> {
    let portal = portal(svc, params.account_id).await?;
    let devices = portal
        .list_devices()
        .await?
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
    Ok(p::PortalDevicesListResult { devices })
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
