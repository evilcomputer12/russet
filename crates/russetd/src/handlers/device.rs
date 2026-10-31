// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `device` handlers (§8.2). Owner: W4 (Wave 1), W10 (Wave 2), W30 (Wi-Fi).
//!
//! Each handler asks the [`crate::DeviceSlot`] for the device manager (which starts it, and so the connector, on
//! the first call that passes the backend and live-device lock checks, ARCHITECTURE §3.3 item 7) and calls
//! `russet_device::rpc`. What needs the connection lives here: the reservation principal (`device.reserved`), the
//! `device.changed` subscription and the end of device-bound leases when their owner goes (+ `lease-grace`).
//! W10's rows (`device.unpair`, `device.developerMode.reveal`, `device.ddi.list`) and W30's `device.wifi.enable`
//! still answer `core.notImplemented`.

use std::sync::Arc;
use std::time::Duration;

use russet_core::Result;
use russet_device::DeviceManager;
use russet_device::rpc::{self, Caller};
use russet_ipc::{Peer, RequestCtx, Router};
use russet_protocol::device as p;

use super::not_implemented;
use crate::DaemonCtx;

/// The device manager of this daemon (started lazily by the slot).
pub(crate) async fn manager(cx: &DaemonCtx) -> Result<Arc<DeviceManager>> {
    cx.devices.manager().await
}

/// The calling connection as the reservation check sees it [D 68].
pub(crate) fn caller(rq: &RequestCtx) -> Caller {
    let conn = rq.peer.connection();
    Caller { conn: conn.id.clone(), shared_lease: conn.shared_lease.clone() }
}

/// Resolves once the connection is gone and the lease grace (ARCHITECTURE §6.3, `[device] lease-grace`) passed.
pub(crate) async fn owner_gone(peer: &Peer, grace: Duration) {
    russet_device::serve::owner_gone(peer.closed(), grace).await;
}

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::DeviceList, _, _>(device_list);
    r.request::<p::DeviceWatch, _, _>(device_watch);
    r.request::<p::DeviceInfo, _, _>(device_info);
    r.request::<p::DeviceReadiness, _, _>(device_readiness);
    r.request::<p::DevicePrepare, _, _>(device_prepare);
    r.request::<p::DevicePair, _, _>(device_pair);
    r.request::<p::DeviceUnpair, _, _>(not_implemented::<p::DeviceUnpair>);
    r.request::<p::DeviceDeveloperModeReveal, _, _>(not_implemented::<p::DeviceDeveloperModeReveal>);
    r.request::<p::DeviceDdiStatus, _, _>(device_ddi_status);
    r.request::<p::DeviceDdiList, _, _>(not_implemented::<p::DeviceDdiList>);
    r.request::<p::DeviceDdiMount, _, _>(device_ddi_mount);
    r.request::<p::DeviceReserve, _, _>(device_reserve);
    r.request::<p::DeviceAliasSet, _, _>(device_alias_set);
    r.request::<p::DeviceWifiEnable, _, _>(not_implemented::<p::DeviceWifiEnable>);
}

async fn device_list(cx: DaemonCtx, _rq: RequestCtx, _p: p::DeviceListParams) -> Result<p::DeviceListResult> {
    rpc::device_list(manager(&cx).await?.as_ref()).await
}

/// `device.watch`: forwards registry events as `device.changed` until the connection closes
/// ([`russet_device::serve::serve_watch`]).
async fn device_watch(cx: DaemonCtx, rq: RequestCtx, _p: p::DeviceWatchParams) -> Result<p::DeviceWatchResult> {
    let m = manager(&cx).await?;
    let subscription_id = russet_device::ids::new_subscription_id();
    let events = m.events();
    let (peer, sub) = (rq.peer.clone(), subscription_id.clone());
    tokio::spawn(async move {
        let notify = |n: p::DeviceChangedParams| peer.notify::<p::DeviceChanged>(n).is_ok();
        russet_device::serve::serve_watch(sub, events, peer.closed(), notify).await;
    });
    Ok(p::DeviceWatchResult { subscription_id })
}

async fn device_info(cx: DaemonCtx, _rq: RequestCtx, p: p::DeviceInfoParams) -> Result<p::DeviceInfoResult> {
    rpc::device_info(manager(&cx).await?.as_ref(), p).await
}

async fn device_readiness(
    cx: DaemonCtx,
    _rq: RequestCtx,
    p: p::DeviceReadinessParams,
) -> Result<p::DeviceReadinessResult> {
    rpc::device_readiness(manager(&cx).await?.as_ref(), p).await
}

async fn device_prepare(cx: DaemonCtx, rq: RequestCtx, p: p::DevicePrepareParams) -> Result<p::DevicePrepareResult> {
    rpc::device_prepare(manager(&cx).await?.as_ref(), &caller(&rq), p).await
}

async fn device_pair(cx: DaemonCtx, rq: RequestCtx, p: p::DevicePairParams) -> Result<p::DevicePairResult> {
    rpc::device_pair(manager(&cx).await?.as_ref(), &caller(&rq), p).await
}

async fn device_ddi_status(
    cx: DaemonCtx,
    _rq: RequestCtx,
    p: p::DeviceDdiStatusParams,
) -> Result<p::DeviceDdiStatusResult> {
    rpc::device_ddi_status(manager(&cx).await?.as_ref(), p).await
}

async fn device_ddi_mount(
    cx: DaemonCtx,
    rq: RequestCtx,
    p: p::DeviceDdiMountParams,
) -> Result<p::DeviceDdiMountResult> {
    rpc::device_ddi_mount(manager(&cx).await?.as_ref(), &caller(&rq), p, &russet_device::NoProgress).await
}

/// `device.reserve`: the reservation ends with `lease.release` or when its owner is gone (+ grace).
async fn device_reserve(cx: DaemonCtx, rq: RequestCtx, p: p::DeviceReserveParams) -> Result<p::DeviceReserveResult> {
    let m = manager(&cx).await?;
    let result = rpc::device_reserve(&m, &caller(&rq), p)?;
    let (peer, lease, grace) = (rq.peer.clone(), result.lease_id.clone(), m.config().lease_grace);
    tokio::spawn(async move {
        russet_device::serve::hold_reservation(m, lease, owner_gone(&peer, grace)).await;
    });
    Ok(result)
}

async fn device_alias_set(
    cx: DaemonCtx,
    _rq: RequestCtx,
    p: p::DeviceAliasSetParams,
) -> Result<p::DeviceAliasSetResult> {
    rpc::device_alias_set(manager(&cx).await?.as_ref(), p)
}
