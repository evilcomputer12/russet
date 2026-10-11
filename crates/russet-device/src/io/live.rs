// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`LiveConnector`]: the real device backend (device.md §4, §5.3, §6.1–§6.2).
//!
//! - Lockdown services: Russet's mux client ([`crate::mux`]) → lockdown `StartSession` with the pair record from
//!   the mux → `StartService` → a second mux connection to the service port → TLS when the service asks for it.
//! - RSD services: one CoreDeviceProxy tunnel per device ([`crate::tunnel`]), opened lazily, with the RSD map of its
//!   generation; a dead tunnel is rebuilt once on the next call, and an idle one (no live stream for
//!   `device.keep-warm`) is closed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use idevice::lockdown::LockdownClient;
use idevice::provider::IdeviceProvider;
use idevice::{IdeviceError, IdeviceService, ReadWrite};
use russet_core::Result;
use russet_core::error::codes;
use serde::Deserialize;
use time::OffsetDateTime;

use super::facade::{self, MuxRecord, PairingStatus, TunnelStatusRecord};
use super::{FacadeCall, ServiceConnector, plist_to_json};
use crate::error::{device_error, from_idevice, invalid_argument};
use crate::manager::{DeviceConfig, DeviceEvent};
use crate::model::{DeviceSummary, Transport, Udid};
use crate::mux::{MuxClient, MuxDevice, MuxEndpoint, MuxEvent, MuxProvider};
use crate::tunnel::{Tunnel, TunnelConfig};

/// The label idevice puts into lockdown requests.
const LABEL: &str = "russet";
/// Reconnect backoff for the mux `Listen` connection (device.md §6.1).
const LISTEN_BACKOFF: [Duration; 4] =
    [Duration::from_millis(250), Duration::from_secs(1), Duration::from_secs(3), Duration::from_secs(10)];

/// The real connector.
#[derive(Debug)]
pub struct LiveConnector {
    endpoint: MuxEndpoint,
    tunnel_cfg: TunnelConfig,
    keep_warm: Duration,
    /// Mux records by UDID (refreshed by `ListDevices` and the watch stream).
    devices: Arc<Mutex<HashMap<Udid, MuxDevice>>>,
    /// One tunnel slot per device.
    tunnels: Arc<Mutex<HashMap<Udid, Arc<TunnelSlot>>>>,
    generation: Arc<AtomicU64>,
}

#[derive(Debug, Default)]
struct TunnelSlot {
    tunnel: tokio::sync::Mutex<Option<Tunnel>>,
    status: Mutex<TunnelStatusRecord>,
    last_used: Mutex<Option<Instant>>,
}

impl TunnelSlot {
    fn touch(&self) {
        if let Ok(mut l) = self.last_used.lock() {
            *l = Some(Instant::now());
        }
    }

    fn set_status(&self, s: TunnelStatusRecord) {
        if let Ok(mut st) = self.status.lock() {
            *st = s;
        }
    }

    fn status(&self) -> TunnelStatusRecord {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UdidArgs {
    udid: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValuesArgs {
    udid: String,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    session: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairArgs {
    udid: String,
    #[serde(default)]
    host_name: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

fn args<T: serde::de::DeserializeOwned>(call: &FacadeCall) -> Result<T> {
    serde_json::from_value(call.args.clone()).map_err(|e| invalid_argument(format!("{}: bad arguments: {e}", call.api)))
}

impl LiveConnector {
    pub(super) fn with_config(cfg: &DeviceConfig) -> Result<Self> {
        let endpoint = MuxEndpoint::from_setting(&cfg.mux)?;
        Ok(Self {
            endpoint,
            tunnel_cfg: cfg.tunnel.clone(),
            keep_warm: cfg.keep_warm,
            devices: Arc::default(),
            tunnels: Arc::default(),
            generation: Arc::new(AtomicU64::new(0)),
        })
    }

    /// The mux endpoint in use.
    pub fn endpoint(&self) -> &MuxEndpoint {
        &self.endpoint
    }

    async fn refresh_devices(&self) -> Result<Vec<MuxDevice>> {
        let mut mux = MuxClient::connect(&self.endpoint).await?;
        let list = mux.list_devices().await?;
        if let Ok(mut map) = self.devices.lock() {
            map.clear();
            for d in &list {
                map.insert(d.udid.clone(), d.clone());
            }
        }
        Ok(list)
    }

    async fn mux_device(&self, udid: &Udid) -> Result<MuxDevice> {
        let cached = self.devices.lock().ok().and_then(|m| m.get(udid).cloned());
        if let Some(d) = cached {
            return Ok(d);
        }
        let list = self.refresh_devices().await?;
        // Prefer USB over Wi-Fi for the same UDID (device.md §6.1).
        list.into_iter()
            .filter(|d| &d.udid == udid)
            .min_by_key(|d| if d.transport == Transport::Usb { 0 } else { 1 })
            .ok_or_else(|| {
                device_error(codes::device::NOT_FOUND, format!("device {} is not attached", udid.redacted()))
            })
    }

    async fn provider(&self, udid: &Udid) -> Result<MuxProvider> {
        let d = self.mux_device(udid).await?;
        Ok(MuxProvider {
            endpoint: self.endpoint.clone(),
            device_id: d.device_id,
            udid: udid.clone(),
            label: LABEL.into(),
        })
    }

    /// Lockdown with a session (when `session`) for `udid`.
    async fn lockdown(&self, provider: &MuxProvider, session: bool) -> Result<LockdownClient> {
        let mut lockdown =
            LockdownClient::connect(provider).await.map_err(|e| from_idevice(e, "connecting to lockdown"))?;
        if session {
            let pairing = provider.pairing_file().await?;
            lockdown.start_session(&pairing).await.map_err(|e| from_idevice(e, "starting a lockdown session"))?;
        }
        Ok(lockdown)
    }

    async fn open_lockdown_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn ReadWrite>> {
        let provider = self.provider(udid).await?;
        let pairing = provider.pairing_file().await?;
        let mut lockdown =
            LockdownClient::connect(&provider).await.map_err(|e| from_idevice(e, "connecting to lockdown"))?;
        lockdown.start_session(&pairing).await.map_err(|e| from_idevice(e, "starting a lockdown session"))?;
        let (port, ssl) =
            lockdown.start_service(name).await.map_err(|e| from_idevice(e, &format!("starting {name}")))?;
        let mut dev = provider.connect(port).await.map_err(|e| from_idevice(e, &format!("connecting to {name}")))?;
        if ssl {
            dev.start_session(&pairing, false).await.map_err(|e| from_idevice(e, &format!("TLS for {name}")))?;
        }
        dev.get_socket().ok_or_else(|| device_error(codes::device::IO, format!("{name}: no socket after TLS")))
    }

    fn slot(&self, udid: &Udid) -> Arc<TunnelSlot> {
        let mut map = match self.tunnels.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        Arc::clone(map.entry(udid.clone()).or_default())
    }

    /// Opens the tunnel of `udid` if needed and runs `f` on it; a dead tunnel is rebuilt once.
    async fn with_tunnel<T, F>(&self, udid: &Udid, what: &str, f: F) -> Result<T>
    where
        F: Fn(&Tunnel) -> futures::future::BoxFuture<'_, std::result::Result<T, crate::tunnel::TunnelConnectError>>,
    {
        let slot = self.slot(udid);
        slot.touch();
        for attempt in 0..2 {
            let mut guard = slot.tunnel.lock().await;
            if guard.is_none() {
                let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
                let provider = self.provider(udid).await?;
                match open_with_retry(&provider, &self.tunnel_cfg, generation).await {
                    Ok(t) => {
                        slot.set_status(TunnelStatusRecord {
                            state: "up".into(),
                            generation: Some(t.generation),
                            since: Some(rfc3339(t.since)),
                            error: None,
                            mss: Some(t.mss as u64),
                        });
                        *guard = Some(t);
                        self.spawn_reaper(udid.clone(), Arc::clone(&slot));
                    }
                    Err(e) => {
                        slot.set_status(TunnelStatusRecord {
                            state: "failed".into(),
                            error: Some(e.to_data()),
                            ..Default::default()
                        });
                        return Err(e);
                    }
                }
            }
            let Some(tunnel) = guard.as_ref() else { continue };
            match f(tunnel).await {
                Ok(v) => {
                    slot.touch();
                    return Ok(v);
                }
                Err(e) if e.is_dead() && attempt == 0 => {
                    tracing::info!(device = %udid.redacted(), "device tunnel died; rebuilding it");
                    if let Some(t) = guard.take() {
                        t.close().await;
                    }
                    slot.set_status(TunnelStatusRecord { state: "down".into(), ..Default::default() });
                }
                Err(e) => return Err(e.into_error(what)),
            }
        }
        Err(device_error(codes::device::TUNNEL_FAILED, format!("{what}: the tunnel could not be rebuilt")))
    }

    /// Closes the tunnel after `keep_warm` without use and without live streams (device.md §5.3 idle close).
    fn spawn_reaper(&self, udid: Udid, slot: Arc<TunnelSlot>) {
        let keep_warm = self.keep_warm;
        let tick = (keep_warm / 4).clamp(Duration::from_millis(250), Duration::from_secs(30));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tick).await;
                let idle =
                    slot.last_used.lock().ok().and_then(|l| *l).map(|t| t.elapsed() >= keep_warm).unwrap_or(true);
                let mut guard = slot.tunnel.lock().await;
                let Some(t) = guard.as_ref() else { return };
                if idle && t.active_streams() == 0 {
                    tracing::debug!(device = %udid.redacted(), generation = t.generation, "closing the idle device tunnel");
                    if let Some(t) = guard.take() {
                        t.close().await;
                    }
                    slot.set_status(TunnelStatusRecord { state: "down".into(), ..Default::default() });
                    return;
                }
            }
        });
    }

    async fn drop_tunnel(&self, udid: &Udid) {
        let slot = self.tunnels.lock().ok().and_then(|mut m| m.remove(udid));
        if let Some(slot) = slot
            && let Some(t) = slot.tunnel.lock().await.take()
        {
            t.close().await;
        }
    }

    async fn lockdown_values(&self, a: ValuesArgs) -> Result<serde_json::Value> {
        let udid = Udid(a.udid);
        let provider = self.provider(&udid).await?;
        let mut lockdown = self.lockdown(&provider, a.session).await?;
        let mut values = serde_json::Map::new();
        for key in &a.keys {
            match lockdown.get_value(Some(key.as_str()), a.domain.as_deref()).await {
                Ok(v) => {
                    values.insert(key.clone(), plist_to_json(&v));
                }
                Err(IdeviceError::UnknownErrorType(_)) | Err(IdeviceError::GetProhibited) => {}
                Err(IdeviceError::UnexpectedResponse(_)) => {}
                Err(e) => return Err(from_idevice(e, &format!("lockdown GetValue {key}"))),
            }
        }
        Ok(serde_json::json!({ "values": values }))
    }

    async fn pairing_status(&self, udid: &Udid) -> Result<PairingStatus> {
        let provider = self.provider(udid).await?;
        let pairing = match provider.pairing_file().await {
            Ok(p) => p,
            Err(e) if e.code() == codes::device::NOT_PAIRED => {
                return Ok(PairingStatus { record_found: false, paired: false, error: Some(e.code().as_str().into()) });
            }
            Err(e) => return Err(e),
        };
        let mut lockdown =
            LockdownClient::connect(&provider).await.map_err(|e| from_idevice(e, "connecting to lockdown"))?;
        match lockdown.start_session(&pairing).await {
            Ok(_) => Ok(PairingStatus { record_found: true, paired: true, error: None }),
            Err(e) => {
                let err = from_idevice(e, "starting a lockdown session");
                Ok(PairingStatus { record_found: true, paired: false, error: Some(err.code().as_str().into()) })
            }
        }
    }

    async fn pair(&self, a: PairArgs) -> Result<serde_json::Value> {
        let udid = Udid(a.udid);
        let device = self.mux_device(&udid).await?;
        let provider = self.provider(&udid).await?;
        // Already paired: nothing to do (device.md §6.2 validates with a session first).
        if self.pairing_status(&udid).await?.paired {
            return Ok(serde_json::json!({ "status": "paired" }));
        }
        let mut mux = MuxClient::connect(&self.endpoint).await?;
        let buid = mux.read_buid().await?;
        let host_id = uuid::Uuid::new_v4().to_string().to_uppercase();
        let mut lockdown =
            LockdownClient::connect(&provider).await.map_err(|e| from_idevice(e, "connecting to lockdown"))?;
        let timeout = Duration::from_millis(a.timeout_ms.unwrap_or(120_000));
        let outcome = tokio::time::timeout(timeout, lockdown.pair(host_id, buid, a.host_name.as_deref())).await;
        let pairing = match outcome {
            Err(_) => return Ok(serde_json::json!({ "status": "pending" })),
            Ok(Err(IdeviceError::UserDeniedPairing)) => return Ok(serde_json::json!({ "status": "denied" })),
            Ok(Err(IdeviceError::PasswordProtected)) => return Ok(serde_json::json!({ "status": "locked" })),
            Ok(Err(e)) => return Err(from_idevice(e, "pairing")),
            Ok(Ok(p)) => p,
        };
        // Validate with a session, then hand the record to the mux so usbmuxd/AMDS own it.
        let mut check =
            LockdownClient::connect(&provider).await.map_err(|e| from_idevice(e, "connecting to lockdown"))?;
        check.start_session(&pairing).await.map_err(|e| from_idevice(e, "validating the new pair record"))?;
        let bytes = pairing.serialize().map_err(|e| from_idevice(e, "serializing the pair record"))?;
        mux.save_pair_record(&udid, device.device_id, bytes).await?;
        Ok(serde_json::json!({ "status": "paired" }))
    }
}

#[async_trait]
impl ServiceConnector for LiveConnector {
    async fn lockdown_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn ReadWrite>> {
        self.open_lockdown_service(udid, name).await
    }

    async fn rsd_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn ReadWrite>> {
        let timeout = self.tunnel_cfg.connect_timeout;
        let name_owned = name.to_owned();
        self.with_tunnel(udid, name, move |t| {
            let name = name_owned.clone();
            Box::pin(async move {
                let port = match t.service_port(&name) {
                    Ok(p) => p,
                    Err(_) => {
                        return Err(crate::tunnel::TunnelConnectError::Refused(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!("the device does not advertise {name}"),
                        )));
                    }
                };
                t.connect_port(port, timeout).await
            })
        })
        .await
        .map_err(|e| {
            if e.message().contains("does not advertise") {
                device_error(codes::device::SERVICE_UNAVAILABLE, e.message().to_owned())
            } else {
                e
            }
        })
    }

    async fn tunnel_port(&self, udid: &Udid, port: u16) -> Result<Box<dyn ReadWrite>> {
        let timeout = self.tunnel_cfg.connect_timeout;
        self.with_tunnel(udid, &format!("tunnel port {port}"), move |t| Box::pin(t.connect_port(port, timeout))).await
    }

    async fn usbmux_port(&self, udid: &Udid, port: u16) -> Result<Box<dyn ReadWrite>> {
        self.provider(udid).await?.connect_raw(port).await
    }

    async fn watch(&self) -> Result<BoxStream<'static, DeviceEvent>> {
        let (tx, rx) = tokio::sync::mpsc::channel::<DeviceEvent>(64);
        let endpoint = self.endpoint.clone();
        let devices = Arc::clone(&self.devices);
        let tunnels = Arc::clone(&self.tunnels);
        tokio::spawn(async move {
            let mut attempt = 0usize;
            loop {
                if tx.is_closed() {
                    return;
                }
                match watch_once(&endpoint, &devices, &tunnels, &tx).await {
                    Ok(()) => attempt = 0,
                    Err(e) => tracing::debug!(error = %e.message(), "mux listen ended"),
                }
                // The mux went away: every device it reported is gone until it comes back.
                let gone: Vec<Udid> =
                    devices.lock().map(|mut m| m.drain().map(|(u, _)| u).collect()).unwrap_or_default();
                for udid in gone {
                    if tx.send(DeviceEvent::Removed(udid)).await.is_err() {
                        return;
                    }
                }
                let delay = LISTEN_BACKOFF[attempt.min(LISTEN_BACKOFF.len() - 1)];
                attempt += 1;
                tokio::time::sleep(delay).await;
            }
        });
        let stream = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) });
        Ok(stream.boxed())
    }

    async fn call(&self, call: FacadeCall) -> Result<serde_json::Value> {
        match call.api.as_str() {
            facade::MUX_LIST => {
                let list = self.refresh_devices().await?;
                let out: Vec<MuxRecord> = list
                    .into_iter()
                    .map(|d| MuxRecord {
                        udid: d.udid.0,
                        device_id: d.device_id,
                        transport: if d.transport == Transport::Usb { "usb".into() } else { "wifi".into() },
                    })
                    .collect();
                Ok(serde_json::to_value(out).unwrap_or_default())
            }
            facade::MUX_PING => {
                let list = self.refresh_devices().await?;
                Ok(serde_json::json!({ "endpoint": self.endpoint.describe(), "devices": list.len() }))
            }
            facade::LOCKDOWN_VALUES => self.lockdown_values(args(&call)?).await,
            facade::PAIRING_STATUS => {
                let a: UdidArgs = args(&call)?;
                Ok(serde_json::to_value(self.pairing_status(&Udid(a.udid)).await?).unwrap_or_default())
            }
            facade::PAIRING_PAIR => self.pair(args(&call)?).await,
            facade::TUNNEL_STATUS => {
                let a: UdidArgs = args(&call)?;
                let slot = self.tunnels.lock().ok().and_then(|m| m.get(&Udid(a.udid)).cloned());
                let status = slot.map(|s| s.status()).filter(|s| !s.state.is_empty());
                let status =
                    status.unwrap_or_else(|| TunnelStatusRecord { state: "down".into(), ..Default::default() });
                Ok(serde_json::to_value(status).unwrap_or_default())
            }
            facade::TUNNEL_CLOSE => {
                let a: UdidArgs = args(&call)?;
                self.drop_tunnel(&Udid(a.udid)).await;
                Ok(serde_json::json!({}))
            }
            facade::TUNNEL_KILL => {
                let a: UdidArgs = args(&call)?;
                let slot = self.tunnels.lock().ok().and_then(|m| m.get(&Udid(a.udid)).cloned());
                let mut killed = false;
                if let Some(slot) = slot
                    && let Some(t) = slot.tunnel.lock().await.as_mut()
                {
                    t.kill().await;
                    killed = true;
                }
                Ok(serde_json::json!({ "killed": killed }))
            }
            facade::RSD_SERVICES => {
                let a: UdidArgs = args(&call)?;
                let udid = Udid(a.udid);
                self.with_tunnel(&udid, "rsd.services", |t| {
                    let services: serde_json::Map<String, serde_json::Value> = t
                        .services
                        .iter()
                        .map(|(k, v)| (k.clone(), serde_json::json!({ "port": v.port, "features": v.features })))
                        .collect();
                    let properties: serde_json::Map<String, serde_json::Value> =
                        t.properties.iter().map(|(k, v)| (k.clone(), plist_to_json(v))).collect();
                    let generation = t.generation;
                    Box::pin(async move {
                        Ok(serde_json::json!({ "generation": generation, "services": services, "properties": properties }))
                    })
                })
                .await
            }
            other => Err(device_error(codes::device::SERVICE_NOT_IMPLEMENTED, format!("unknown facade call {other}"))),
        }
    }

    fn backend(&self) -> &'static str {
        "live"
    }
}

/// Backoff for CoreDeviceProxy opens the device refuses with an early EOF. The device tears a closed tunnel down
/// lazily: back-to-back open/close cycles hit its limit after about two dozen tunnels in a few seconds (measured on
/// iOS 27.0, 2026-10-11) and it accepts new ones again shortly after.
const OPEN_BACKOFF: [Duration; 5] = [
    Duration::from_millis(150),
    Duration::from_millis(300),
    Duration::from_millis(600),
    Duration::from_millis(1200),
    Duration::from_millis(2400),
];

async fn open_with_retry(provider: &MuxProvider, cfg: &TunnelConfig, generation: u64) -> Result<Tunnel> {
    let mut attempt = 0;
    loop {
        match Tunnel::open_usb(provider, cfg, generation).await {
            Ok(t) => return Ok(t),
            Err(e)
                if attempt < OPEN_BACKOFF.len()
                    && [codes::device::DISCONNECTED, codes::device::TUNNEL_FAILED, codes::device::IO]
                        .contains(&e.code()) =>
            {
                tracing::debug!(attempt, error = %e.message(), "CoreDeviceProxy refused the tunnel; retrying");
                tokio::time::sleep(OPEN_BACKOFF[attempt]).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// One mux `Listen` session: the snapshot first, then events until the connection drops.
async fn watch_once(
    endpoint: &MuxEndpoint,
    devices: &Arc<Mutex<HashMap<Udid, MuxDevice>>>,
    tunnels: &Arc<Mutex<HashMap<Udid, Arc<TunnelSlot>>>>,
    tx: &tokio::sync::mpsc::Sender<DeviceEvent>,
) -> Result<()> {
    let mut listen = MuxClient::connect(endpoint).await?.listen().await?;
    loop {
        // Stop listening as soon as nobody watches (the manager is gone), not at the next mux event.
        let ev = tokio::select! {
            _ = tx.closed() => return Ok(()),
            ev = listen.next() => ev,
        };
        let Some(ev) = ev else { break };
        match ev? {
            MuxEvent::Attached(d) => {
                let known = devices.lock().map(|m| m.contains_key(&d.udid)).unwrap_or(false);
                if let Ok(mut m) = devices.lock() {
                    // USB wins over Wi-Fi for the same UDID.
                    let keep_old = m
                        .get(&d.udid)
                        .is_some_and(|old| old.transport == Transport::Usb && d.transport != Transport::Usb);
                    if !keep_old {
                        m.insert(d.udid.clone(), d.clone());
                    }
                }
                if !known && tx.send(DeviceEvent::Added(DeviceSummary::attached(d.udid, d.transport))).await.is_err() {
                    return Ok(());
                }
            }
            MuxEvent::Detached(id) => {
                let udid = devices.lock().ok().and_then(|mut m| {
                    let udid = m.iter().find(|(_, d)| d.device_id == id).map(|(u, _)| u.clone())?;
                    m.remove(&udid);
                    Some(udid)
                });
                if let Some(udid) = udid {
                    let slot = tunnels.lock().ok().and_then(|mut m| m.remove(&udid));
                    if let Some(slot) = slot
                        && let Some(t) = slot.tunnel.lock().await.take()
                    {
                        t.close().await;
                    }
                    if tx.send(DeviceEvent::Removed(udid)).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
    Ok(())
}
