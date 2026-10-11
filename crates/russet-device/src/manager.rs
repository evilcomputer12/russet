// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`DeviceManager`] (one actor per UDID) and the cloneable [`DeviceHandle`] (ARCHITECTURE §5.3).
//!
//! The manager keeps the device registry from the connector's watch stream and starts reading a device's identity
//! only when a client asks (`device.list`, `device.info`, a `device.watch` subscriber, or any device operation),
//! so a daemon nobody asks never talks to the phone [D 63, D 83].

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use futures::StreamExt;
use russet_core::Result;
use russet_core::error::codes;
use russet_core::ids::LeaseId;
use russet_core::progress::ProgressSink;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::actor::DeviceActor;
use crate::apps::{AppFilter, AppInfo, InstallOptions, InstalledApp};
use crate::crash::{CrashQuery, IpsReport};
use crate::debug::{BindSpec, ForwardHandle, ForwardInfo, ForwardRegistry, ForwardTarget};
use crate::error::{device_error, invalid_argument};
use crate::io::facade::{self, MuxRecord};
use crate::io::{FacadeCall, ServiceConnector};
use crate::logs::{LogFilter, LogStream};
use crate::model::{DeviceInfo, DeviceSelector, DeviceSummary, Readiness, Transport, Udid};
use crate::perf::{SysmonOptions, SysmonStream};
use crate::process::{LaunchRequest, LaunchedProcess, ProcessInfo};
use crate::screen::{InputEvent, MirrorHandle, MirrorOptions, Screenshot};

pub use crate::config::{DdiConfig, DeviceConfig};

/// A device-registry event.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceEvent {
    Added(DeviceSummary),
    Changed(DeviceSummary),
    Removed(Udid),
}

/// An exclusive device reservation (`device.reserve`, ARCHITECTURE §6.3) [D 68].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub lease: LeaseId,
    /// The connection id (`c-7`) that made it.
    pub owner: String,
    pub purpose: String,
}

/// State shared by the manager and every handle.
pub(crate) struct Shared {
    pub cfg: DeviceConfig,
    pub conn: Arc<dyn ServiceConnector>,
    pub devices: RwLock<BTreeMap<Udid, Arc<DeviceActor>>>,
    pub events: broadcast::Sender<DeviceEvent>,
    pub aliases: RwLock<BTreeMap<String, String>>,
    pub forwards: ForwardRegistry,
    pub reservations: Mutex<BTreeMap<Udid, Reservation>>,
    /// The last mux error, when the registry snapshot failed.
    pub mux_error: Mutex<Option<russet_core::error::ErrorData>>,
    pub shutdown: CancellationToken,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").field("backend", &self.conn.backend()).finish_non_exhaustive()
    }
}

impl Shared {
    pub fn actor(&self, udid: &Udid) -> Option<Arc<DeviceActor>> {
        self.devices.read().ok().and_then(|m| m.get(udid).cloned())
    }

    pub fn broadcast(&self, ev: DeviceEvent) {
        let _ = self.events.send(ev);
    }

    fn insert(&self, udid: Udid, transport: Transport) -> (Arc<DeviceActor>, bool) {
        let mut map = match self.devices.write() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        if let Some(a) = map.get(&udid) {
            if let Ok(mut t) = a.transport.lock() {
                *t = transport;
            }
            return (Arc::clone(a), false);
        }
        let a = Arc::new(DeviceActor::new(udid.clone(), transport));
        map.insert(udid, Arc::clone(&a));
        (a, true)
    }

    fn remove(&self, udid: &Udid) -> bool {
        let removed = self.devices.write().ok().and_then(|mut m| m.remove(udid)).is_some();
        if removed {
            self.forwards.close_device(udid);
            if let Ok(mut r) = self.reservations.lock() {
                r.remove(udid);
            }
        }
        removed
    }
}

/// The device registry: one actor per attached UDID.
#[derive(Debug)]
pub struct DeviceManager {
    shared: Arc<Shared>,
}

static ACTIVE: Mutex<Option<Weak<DeviceManager>>> = Mutex::new(None);

impl DeviceManager {
    pub async fn start(cfg: DeviceConfig, conn: Arc<dyn ServiceConnector>) -> Result<Arc<Self>> {
        let (events, _) = broadcast::channel(256);
        let aliases = RwLock::new(cfg.aliases.clone());
        let shared = Arc::new(Shared {
            cfg,
            conn,
            devices: RwLock::new(BTreeMap::new()),
            events,
            aliases,
            forwards: ForwardRegistry::default(),
            reservations: Mutex::new(BTreeMap::new()),
            mux_error: Mutex::new(None),
            shutdown: CancellationToken::new(),
        });
        snapshot(&shared).await;
        let stream = shared.conn.watch().await?;
        let weak = Arc::downgrade(&shared);
        let stop = shared.shutdown.clone();
        tokio::spawn(async move {
            let mut stream = stream;
            loop {
                let ev = tokio::select! {
                    _ = stop.cancelled() => return,
                    ev = stream.next() => ev,
                };
                let Some(ev) = ev else { return };
                let Some(shared) = weak.upgrade() else { return };
                apply(&shared, ev);
            }
        });
        let manager = Arc::new(Self { shared });
        if let Ok(mut a) = ACTIVE.lock() {
            *a = Some(Arc::downgrade(&manager));
        }
        Ok(manager)
    }

    /// The manager the daemon started last, for doctor checks (`None` before the first device call).
    pub fn active() -> Option<Arc<DeviceManager>> {
        ACTIVE.lock().ok().and_then(|a| a.as_ref().and_then(Weak::upgrade))
    }

    /// The cached registry (no device I/O).
    pub fn devices(&self) -> Vec<DeviceSummary> {
        let actors: Vec<Arc<DeviceActor>> =
            self.shared.devices.read().map(|m| m.values().cloned().collect()).unwrap_or_default();
        actors.iter().map(|a| a.summary()).collect()
    }

    /// `device.list`: the registry with every device's identity read (lazily, at most `budget` per call). Fails with
    /// `device.usbmuxUnavailable` when the mux cannot be reached and no device is known.
    pub async fn list(&self, budget: Duration) -> Result<Vec<DeviceSummary>> {
        if self.shared.devices.read().map(|m| m.is_empty()).unwrap_or(true) {
            snapshot(&self.shared).await;
            let err = self.shared.mux_error.lock().ok().and_then(|e| e.clone());
            if let Some(err) = err {
                return Err(device_error(codes::device::USBMUX_UNAVAILABLE, "the mux daemon is not reachable")
                    .with_details(serde_json::json!({ "cause": err.russet_code })));
            }
        }
        let handles: Vec<DeviceHandle> = self
            .shared
            .devices
            .read()
            .map(|m| {
                m.values().map(|a| DeviceHandle { shared: Arc::clone(&self.shared), actor: Arc::clone(a) }).collect()
            })
            .unwrap_or_default();
        let loads = handles.iter().map(|h| async move {
            let _ = tokio::time::timeout(budget, h.ensure_identity()).await;
        });
        futures::future::join_all(loads).await;
        Ok(self.devices())
    }

    pub fn events(&self) -> broadcast::Receiver<DeviceEvent> {
        self.shared.events.subscribe()
    }

    /// Resolves a selector: a UDID, an alias (`[devices.aliases]`), or the only attached device.
    pub fn get(&self, sel: &DeviceSelector) -> Result<DeviceHandle> {
        let actor = match sel {
            DeviceSelector::Udid(u) => self.shared.actor(u).ok_or_else(|| {
                device_error(codes::device::NOT_FOUND, format!("device {} is not attached", u.redacted()))
            })?,
            DeviceSelector::Alias(a) => {
                let udid = self.shared.aliases.read().ok().and_then(|m| m.get(a).cloned()).ok_or_else(|| {
                    device_error(codes::device::NOT_FOUND, format!("no device alias {a:?}"))
                        .with_hint("Set one with `russet device alias <alias> --device <udid>`, or pass the UDID.")
                })?;
                self.shared.actor(&Udid(udid)).ok_or_else(|| {
                    device_error(codes::device::NOT_FOUND, format!("the device of alias {a:?} is not attached"))
                })?
            }
            DeviceSelector::Only => {
                let actors: Vec<Arc<DeviceActor>> =
                    self.shared.devices.read().map(|m| m.values().cloned().collect()).unwrap_or_default();
                let usable: Vec<Arc<DeviceActor>> = actors
                    .iter()
                    .filter(|a| !matches!(a.cached().state, crate::model::DeviceState::Unavailable { .. }))
                    .cloned()
                    .collect();
                match usable.len() {
                    0 => return Err(device_error(codes::device::NOT_FOUND, "no iPhone is attached")),
                    1 => Arc::clone(&usable[0]),
                    n => {
                        return Err(invalid_argument(format!("{n} devices are attached; say which one"))
                            .with_hint("Pass `--device <udid|alias>` (`russet device list` shows them)."));
                    }
                }
            }
        };
        Ok(DeviceHandle { shared: Arc::clone(&self.shared), actor })
    }

    /// The configuration this manager runs with.
    pub fn config(&self) -> &DeviceConfig {
        &self.shared.cfg
    }

    /// The connector (for the debug plane and tests).
    pub fn connector(&self) -> Arc<dyn ServiceConnector> {
        Arc::clone(&self.shared.conn)
    }

    /// Every open forward (`forward.list`).
    pub fn forwards(&self) -> Vec<ForwardInfo> {
        self.shared.forwards.list()
    }

    /// Closes a forward by lease (`forward.close`); `core.notFound` when none matches.
    pub fn close_forward(&self, lease: &LeaseId) -> Result<()> {
        if self.shared.forwards.close(lease) {
            Ok(())
        } else {
            Err(russet_core::Error::new(codes::core::NOT_FOUND, format!("no forward with lease {lease}")))
        }
    }

    /// `device.alias.set`: records the alias in `[devices.aliases]` of the global config and in this manager.
    pub fn set_alias(&self, alias: &str, udid: &Udid) -> Result<std::path::PathBuf> {
        let path = crate::config::set_alias(&self.shared.cfg.config_dir, alias, udid.as_str())?;
        if let Ok(mut m) = self.shared.aliases.write() {
            if udid.as_str().is_empty() {
                m.remove(alias);
            } else {
                m.insert(alias.to_owned(), udid.as_str().to_owned());
            }
        }
        Ok(path)
    }

    /// Stops the watch task and closes every forward.
    pub fn shutdown(&self) {
        self.shared.shutdown.cancel();
        self.shared.forwards.close_all();
    }

    /// Where Russet keeps per-device state (`state/devices/<udid>/`).
    pub fn device_state_dir(&self, udid: &Udid) -> std::path::PathBuf {
        self.shared.cfg.device_state_dir(udid.as_str())
    }

    /// The runtime directory for forward sockets.
    pub fn runtime_dir(&self) -> &Path {
        &self.shared.cfg.runtime_dir
    }

    /// Makes `owner` the exclusive principal of the device (`device.reserve`).
    pub fn reserve(&self, h: &DeviceHandle, owner: &str, purpose: &str) -> Result<Reservation> {
        let mut map = self
            .shared
            .reservations
            .lock()
            .map_err(|_| russet_core::Error::new(codes::core::INTERNAL, "the reservation table is poisoned"))?;
        if let Some(r) = map.get(h.udid()) {
            if r.owner != owner {
                return Err(device_error(
                    codes::device::RESERVED,
                    format!("{} is reserved by another client ({})", h.udid().redacted(), r.purpose),
                )
                .with_details(serde_json::json!({ "leaseId": r.lease, "purpose": r.purpose })));
            }
            return Ok(r.clone());
        }
        let r = Reservation { lease: crate::ids::new_lease_id(), owner: owner.to_owned(), purpose: purpose.to_owned() };
        map.insert(h.udid().clone(), r.clone());
        Ok(r)
    }

    /// Ends a reservation by lease; `false` when none matches.
    pub fn release(&self, lease: &LeaseId) -> bool {
        self.shared
            .reservations
            .lock()
            .map(|mut m| {
                let before = m.len();
                m.retain(|_, r| &r.lease != lease);
                before != m.len()
            })
            .unwrap_or(false)
    }

    /// `device.reserved` (3018) unless `conn` is the reservation's principal or shares its lease [D 68].
    pub fn check_access(&self, udid: &Udid, conn: &str, shared_lease: Option<&LeaseId>) -> Result<()> {
        let r = self.shared.reservations.lock().ok().and_then(|m| m.get(udid).cloned());
        match r {
            Some(r) if r.owner != conn && shared_lease != Some(&r.lease) => Err(device_error(
                codes::device::RESERVED,
                format!("{} is reserved by another client ({})", udid.redacted(), r.purpose),
            )
            .with_details(serde_json::json!({ "leaseId": r.lease, "purpose": r.purpose }))),
            _ => Ok(()),
        }
    }
}

impl Drop for DeviceManager {
    fn drop(&mut self) {
        self.shared.shutdown.cancel();
    }
}

async fn snapshot(shared: &Arc<Shared>) {
    match shared.conn.call(FacadeCall::new(facade::MUX_LIST, serde_json::json!({}))).await {
        Ok(v) => {
            if let Ok(mut e) = shared.mux_error.lock() {
                *e = None;
            }
            let records: Vec<MuxRecord> = serde_json::from_value(v).unwrap_or_default();
            for r in records {
                let transport = if r.transport == "wifi" { Transport::Wifi } else { Transport::Usb };
                let (actor, new) = shared.insert(Udid(r.udid), transport);
                if new {
                    shared.broadcast(DeviceEvent::Added(actor.summary()));
                }
            }
        }
        Err(e) => {
            if e.code() == codes::device::USBMUX_UNAVAILABLE {
                tracing::warn!(error = %e.message(), "the mux daemon is not reachable");
                if let Ok(mut m) = shared.mux_error.lock() {
                    *m = Some(e.to_data());
                }
            } else {
                tracing::debug!(error = %e.message(), "mux.list is not available on this backend");
            }
        }
    }
}

fn apply(shared: &Arc<Shared>, ev: DeviceEvent) {
    match ev {
        DeviceEvent::Added(s) | DeviceEvent::Changed(s) => {
            let (actor, new) = shared.insert(s.udid.clone(), s.transport);
            if let Ok(mut e) = shared.mux_error.lock() {
                *e = None;
            }
            if s.identity.is_some() || s.readiness.is_some() {
                actor.update(|c| {
                    if s.identity.is_some() {
                        c.identity = s.identity.clone();
                        c.state = s.state.clone();
                    }
                    if s.readiness.is_some() {
                        c.readiness = s.readiness.clone();
                    }
                });
            }
            let summary = actor.summary();
            shared.broadcast(if new { DeviceEvent::Added(summary) } else { DeviceEvent::Changed(summary) });
            // A `device.watch` subscriber counts as a client asking: read the identity in the background.
            if new && shared.events.receiver_count() > 0 {
                let h = DeviceHandle { shared: Arc::clone(shared), actor };
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(5), h.ensure_identity()).await;
                });
            }
        }
        DeviceEvent::Removed(udid) => {
            if shared.remove(&udid) {
                shared.broadcast(DeviceEvent::Removed(udid));
            }
        }
    }
}

/// A cloneable handle; every call works on the device's actor.
#[derive(Clone)]
pub struct DeviceHandle {
    pub(crate) shared: Arc<Shared>,
    pub(crate) actor: Arc<DeviceActor>,
}

impl std::fmt::Debug for DeviceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceHandle").field("udid", &self.actor.udid.redacted()).finish()
    }
}

/// W10 fills screen, mirror, input, crash pull and sysmon; until then they answer this.
fn wave2(what: &str) -> russet_core::Error {
    device_error(codes::device::SERVICE_NOT_IMPLEMENTED, format!("{what} lands in Wave 2 (W10)"))
        .with_hint("This capability is not implemented yet in this Russet build.")
}

impl DeviceHandle {
    /// The device's UDID.
    pub fn udid(&self) -> &Udid {
        &self.actor.udid
    }

    /// The cached summary (no device I/O).
    pub fn summary(&self) -> DeviceSummary {
        self.actor.summary()
    }

    pub(crate) fn conn(&self) -> &Arc<dyn ServiceConnector> {
        &self.shared.conn
    }

    pub(crate) fn cfg(&self) -> &DeviceConfig {
        &self.shared.cfg
    }

    pub(crate) fn changed(&self) {
        self.shared.broadcast(DeviceEvent::Changed(self.actor.summary()));
    }

    pub async fn info(&self) -> Result<DeviceInfo> {
        crate::pairing::info(self).await
    }

    pub async fn readiness(&self, refresh: bool) -> Result<Readiness> {
        crate::pairing::readiness(self, refresh).await
    }

    /// Installs a `.app` or `.ipa`.
    pub async fn install(&self, pkg: &Path, o: InstallOptions, p: &dyn ProgressSink) -> Result<InstalledApp> {
        crate::apps::install(self, pkg, o, p).await
    }

    pub async fn apps(&self, f: AppFilter) -> Result<Vec<AppInfo>> {
        crate::apps::list(self, &f).await
    }

    pub async fn uninstall(&self, bundle_id: &str) -> Result<()> {
        crate::apps::uninstall(self, bundle_id).await
    }

    /// CoreDevice launch with the DVT fallback; keeps the launch channel open (ARCHITECTURE §17.1).
    pub async fn launch(&self, r: LaunchRequest) -> Result<LaunchedProcess> {
        crate::process::launch(self, r).await
    }

    pub async fn signal(&self, pid: u32, sig: i32) -> Result<()> {
        crate::process::signal(self, pid, sig).await
    }

    pub async fn processes(&self) -> Result<Vec<ProcessInfo>> {
        crate::process::list(self).await
    }

    /// A raw stream inside the tunnel (broker, symbols).
    pub async fn connect_rsd(&self, service: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        self.conn().rsd_service(self.udid(), service).await
    }

    pub async fn connect_lockdown(&self, service: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        self.conn().lockdown_service(self.udid(), service).await
    }

    pub async fn forward(&self, t: ForwardTarget, b: BindSpec) -> Result<ForwardHandle> {
        crate::debug::open_forward(self, t, b).await
    }

    pub async fn logs(&self, f: LogFilter) -> Result<LogStream> {
        crate::logs::open(self, f).await
    }

    pub async fn screenshot(&self) -> Result<Screenshot> {
        Err(wave2("screen.screenshot"))
    }

    pub async fn mirror(&self, o: MirrorOptions) -> Result<MirrorHandle> {
        let _ = o;
        Err(wave2("screen.mirror"))
    }

    pub async fn input(&self, e: InputEvent) -> Result<()> {
        let _ = e;
        Err(wave2("input"))
    }

    pub async fn crash_pull(&self, q: CrashQuery, dest: &Path) -> Result<Vec<IpsReport>> {
        let _ = (q, dest);
        Err(wave2("crash pull"))
    }

    pub async fn sysmon(&self, o: SysmonOptions) -> Result<SysmonStream> {
        let _ = o;
        Err(wave2("perf"))
    }
}
