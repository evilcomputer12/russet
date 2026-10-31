// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A minimal in-memory [`ServiceConnector`] for russet-device's own integration tests. russet-testkit's
//! `ScriptedConnector` (W8) is the general one; this answers just the facade calls and streams these tests use.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_device::io::{FacadeCall, ServiceConnector, facade};
use russet_device::model::{DeviceSummary, Transport, Udid};
use russet_device::{DeviceEvent, ReadWriteBox};
use serde_json::json;
use tokio::sync::mpsc;

pub const UDID_A: &str = "00008101-000A1B2C3D4E5F60";
pub const UDID_B: &str = "00008110-0011223344556677";

/// One fake device.
#[derive(Debug, Clone)]
pub struct FakeDevice {
    pub udid: String,
    pub paired: bool,
    pub product_type: String,
    pub os_version: String,
    pub chip_id: u64,
}

impl FakeDevice {
    pub fn iphone12(udid: &str) -> Self {
        Self {
            udid: udid.into(),
            paired: true,
            product_type: "iPhone13,2".into(),
            os_version: "27.0".into(),
            chip_id: 0x8101,
        }
    }
}

/// The fake connector.
pub struct FakeConnector {
    pub devices: Mutex<BTreeMap<String, FakeDevice>>,
    pub events_tx: Mutex<Option<mpsc::UnboundedSender<DeviceEvent>>>,
    events_rx: Mutex<Option<mpsc::UnboundedReceiver<DeviceEvent>>>,
    /// How many `lockdown.values` calls were made (lazy identity checks).
    pub value_calls: AtomicUsize,
    /// Every facade API called, in order.
    pub calls: Mutex<Vec<String>>,
    /// While set, RSD services other than `test.echo` fail with `device.locked` (a locked iPhone).
    pub locked: AtomicBool,
    /// While set, the connector answers like russet-testkit's scripted backend before it adopted the device
    /// plane's catalog: `lockdown.get_value {udid?, key?, domain?}` and `ddi.status` only,
    /// `device.serviceNotImplemented` for the rest.
    pub qa_catalog_only: AtomicBool,
}

impl FakeConnector {
    pub fn new(devices: Vec<FakeDevice>) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            devices: Mutex::new(devices.into_iter().map(|d| (d.udid.clone(), d)).collect()),
            events_tx: Mutex::new(Some(tx)),
            events_rx: Mutex::new(Some(rx)),
            value_calls: AtomicUsize::new(0),
            calls: Mutex::new(Vec::new()),
            locked: AtomicBool::new(false),
            qa_catalog_only: AtomicBool::new(false),
        })
    }

    /// Simulates a mux attach.
    pub fn attach(&self, d: FakeDevice) {
        let udid = d.udid.clone();
        self.devices.lock().expect("lock").insert(udid.clone(), d);
        if let Some(tx) = self.events_tx.lock().expect("lock").as_ref() {
            let _ = tx.send(DeviceEvent::Added(DeviceSummary::attached(Udid(udid), Transport::Usb)));
        }
    }

    /// Simulates a mux detach.
    pub fn detach(&self, udid: &str) {
        self.devices.lock().expect("lock").remove(udid);
        if let Some(tx) = self.events_tx.lock().expect("lock").as_ref() {
            let _ = tx.send(DeviceEvent::Removed(Udid(udid.into())));
        }
    }

    fn device(&self, udid: &str) -> Result<FakeDevice> {
        self.devices
            .lock()
            .expect("lock")
            .get(udid)
            .cloned()
            .ok_or_else(|| Error::new(codes::device::NOT_FOUND, "not attached"))
    }
}

/// An echo service: everything written comes back.
fn echo_stream() -> ReadWriteBox {
    let (client, mut server) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let (mut r, mut w) = tokio::io::split(&mut server);
        let _ = tokio::io::copy(&mut r, &mut w).await;
    });
    Box::new(client)
}

#[async_trait]
impl ServiceConnector for FakeConnector {
    async fn lockdown_service(&self, udid: &Udid, name: &str) -> Result<ReadWriteBox> {
        self.device(udid.as_str())?;
        match name {
            "test.echo" => Ok(echo_stream()),
            _ => Err(Error::new(codes::device::SERVICE_UNAVAILABLE, format!("fake has no lockdown service {name}"))),
        }
    }

    async fn rsd_service(&self, udid: &Udid, name: &str) -> Result<ReadWriteBox> {
        self.device(udid.as_str())?;
        match name {
            "test.echo" => Ok(echo_stream()),
            _ if self.locked.load(Ordering::SeqCst) => {
                Err(Error::new(codes::device::LOCKED, "the iPhone is locked (fake)"))
            }
            _ => Err(Error::new(codes::device::SERVICE_UNAVAILABLE, format!("fake has no RSD service {name}"))),
        }
    }

    async fn tunnel_port(&self, udid: &Udid, _port: u16) -> Result<ReadWriteBox> {
        self.device(udid.as_str())?;
        Ok(echo_stream())
    }

    async fn watch(&self) -> Result<BoxStream<'static, DeviceEvent>> {
        let rx = self.events_rx.lock().expect("lock").take().expect("watch is called once");
        Ok(futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) }).boxed())
    }

    async fn call(&self, call: FacadeCall) -> Result<serde_json::Value> {
        self.calls.lock().expect("lock").push(call.api.clone());
        let udid = call.args.get("udid").and_then(|u| u.as_str()).unwrap_or_default().to_owned();
        if self.qa_catalog_only.load(Ordering::SeqCst) {
            // A scripted scenario's DDI (the optional `ddi.status` call).
            if call.api == facade::DDI_STATUS {
                self.device(&udid)?;
                return Ok(json!({"mounted": true, "kind": "cryptex1", "version": "27.1.266.1",
                                 "productBuild": "27A266a", "source": "dir"}));
            }
            // The shape of russet-testkit's `lockdown_value` (W8, scripted.rs): the whole domain without `key`.
            if call.api != facade::LOCKDOWN_GET_VALUE {
                return Err(Error::new(
                    codes::device::SERVICE_NOT_IMPLEMENTED,
                    format!("the scenario scripts no answer for {}", call.api),
                ));
            }
            let d = self.device(&udid)?;
            let all = json!({"UniqueDeviceID": d.udid, "DeviceName": "Test iPhone", "ProductType": d.product_type,
                             "ProductVersion": d.os_version, "BuildVersion": "24A437", "DeviceClass": "iPhone",
                             "ChipID": d.chip_id, "CPUArchitecture": "arm64e"});
            return match call.args.get("key").and_then(|k| k.as_str()) {
                None => Ok(all),
                Some(k) => all
                    .get(k)
                    .cloned()
                    .ok_or_else(|| Error::new(codes::device::PROTOCOL, format!("lockdown has no value {k}"))),
            };
        }
        match call.api.as_str() {
            facade::MUX_LIST => {
                let list: Vec<_> = self
                    .devices
                    .lock()
                    .expect("lock")
                    .values()
                    .enumerate()
                    .map(|(i, d)| json!({"udid": d.udid, "deviceId": i + 1, "transport": "usb"}))
                    .collect();
                Ok(json!(list))
            }
            facade::LOCKDOWN_VALUES => {
                self.value_calls.fetch_add(1, Ordering::SeqCst);
                let d = self.device(&udid)?;
                let session = call.args.get("session").and_then(|s| s.as_bool()).unwrap_or(false);
                if session && !d.paired {
                    return Err(Error::new(codes::device::NOT_PAIRED, "InvalidHostID"));
                }
                let mut values = json!({"ProductType": d.product_type, "ProductVersion": d.os_version, "BuildVersion": "24A437",
                                        "DeviceClass": "iPhone", "DeviceName": "Test iPhone"});
                if session {
                    values["ChipID"] = json!(d.chip_id);
                    values["CPUArchitecture"] = json!("arm64e");
                }
                Ok(json!({"values": values}))
            }
            facade::PAIRING_STATUS => {
                let d = self.device(&udid)?;
                Ok(json!({"recordFound": d.paired, "paired": d.paired}))
            }
            facade::TUNNEL_STATUS => Ok(json!({"state": "down"})),
            facade::MUX_PING => Ok(json!({"endpoint": "fake", "devices": self.devices.lock().expect("lock").len()})),
            other => Err(Error::new(codes::device::SERVICE_NOT_IMPLEMENTED, format!("fake: {other}"))),
        }
    }

    fn backend(&self) -> &'static str {
        "memory"
    }
}
