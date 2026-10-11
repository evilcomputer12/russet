// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The per-device state behind a [`crate::DeviceHandle`] (device.md §4: one actor per UDID). Identity, readiness
//! and the lock hint are cached here until detach; identity reads are single-flight. Device I/O itself goes
//! through the connector; the tunnel lives in the live connector.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::logs::LogCapture;
use crate::model::{DeviceState, DeviceSummary, Identity, Readiness, Transport, Tri, Udid};
use crate::process::OutputHub;

/// What the device actor caches.
#[derive(Debug, Clone, Default)]
pub(crate) struct Cached {
    pub identity: Option<Identity>,
    pub state: DeviceState,
    pub readiness: Option<Readiness>,
    /// Set by errors that say "Locked"; cleared by the next successful launch (device.md F9).
    pub locked: Tri,
}

/// One attached device.
#[derive(Debug)]
pub(crate) struct DeviceActor {
    pub udid: Udid,
    pub transport: Mutex<Transport>,
    pub cached: RwLock<Cached>,
    /// Single-flight identity and readiness reads.
    pub identity_lock: tokio::sync::Mutex<()>,
    /// Serializes DDI mounts and pairing (ARCHITECTURE §6.2 `LockKey::Device`).
    pub device_lock: tokio::sync::Mutex<()>,
    /// Console output of the processes Russet launched, by pid (the `stdout` log source).
    pub outputs: Mutex<BTreeMap<u32, Arc<OutputHub>>>,
    /// The background capture behind `logs.tail`, by key (`pid:<n>`, `bundle:<id>` or `all`).
    pub captures: Mutex<BTreeMap<String, Arc<LogCapture>>>,
}

impl DeviceActor {
    pub fn new(udid: Udid, transport: Transport) -> Self {
        Self {
            udid,
            transport: Mutex::new(transport),
            cached: RwLock::new(Cached::default()),
            identity_lock: tokio::sync::Mutex::new(()),
            device_lock: tokio::sync::Mutex::new(()),
            outputs: Mutex::new(BTreeMap::new()),
            captures: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn cached(&self) -> Cached {
        match self.cached.read() {
            Ok(c) => c.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    pub fn update(&self, f: impl FnOnce(&mut Cached)) {
        match self.cached.write() {
            Ok(mut c) => f(&mut c),
            Err(p) => f(&mut p.into_inner()),
        }
    }

    pub fn transport(&self) -> Transport {
        self.transport.lock().map(|t| *t).unwrap_or(Transport::Usb)
    }

    pub fn summary(&self) -> DeviceSummary {
        let c = self.cached();
        let mut readiness = c.readiness;
        if let Some(r) = readiness.as_mut() {
            r.locked = c.locked;
        }
        DeviceSummary {
            udid: self.udid.clone(),
            transport: self.transport(),
            identity: c.identity,
            state: c.state,
            readiness,
        }
    }

    /// Records the console of a launched process for the `stdout` log source; finished ones are pruned.
    pub fn register_output(&self, pid: u32, hub: Arc<OutputHub>) {
        if let Ok(mut m) = self.outputs.lock() {
            m.retain(|_, h| !h.is_finished());
            m.insert(pid, hub);
        }
    }

    pub fn output(&self, pid: u32) -> Option<Arc<OutputHub>> {
        self.outputs.lock().ok().and_then(|m| m.get(&pid).cloned())
    }
}
