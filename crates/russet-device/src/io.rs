// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The device I/O seam (testability contract T5, ARCHITECTURE §5.3 `io`): plaintext service streams after TLS
//! (lockdown) or inside the tunnel (RSD). Scripted, replay and recording connectors live in russet-testkit.
//!
//! Everything the device plane does goes through a [`ServiceConnector`]:
//! - **streams**: [`ServiceConnector::lockdown_service`] and [`ServiceConnector::rsd_service`] return plaintext
//!   streams on which russet-device builds idevice's clients (`Idevice::new(stream, label)`,
//!   `RsdService::from_stream`), so a replay connector can serve recorded bytes (QA §3.5);
//! - **facade calls**: [`ServiceConnector::call`] answers the device-level questions that are not a service stream
//!   (mux list, lockdown values, pairing state, tunnel state). The catalog is [`facade`]; each API documents its
//!   `args` and result JSON, so russet-testkit's scripted connector can answer them.
//!
//! [`LiveConnector`] is the real implementation: usbmuxd (Russet's own client, [`crate::mux`]), lockdown with
//! TLS, and the CoreDeviceProxy tunnel with the MSS clamp ([`crate::tunnel`]).

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream::BoxStream;
use russet_core::Result;
use russet_core::error::codes;

use crate::error::device_error;
use crate::manager::{DeviceConfig, DeviceEvent};
use crate::model::Udid;

mod live;

/// A facade-level call, for recording and replay at the API level (QA §3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacadeCall {
    pub api: String,
    pub args: serde_json::Value,
}

impl FacadeCall {
    /// A call of `api` with `args`.
    pub fn new(api: &str, args: serde_json::Value) -> Self {
        Self { api: api.to_owned(), args }
    }
}

/// Opens device service streams and watches attach/detach.
#[async_trait]
pub trait ServiceConnector: Send + Sync + 'static {
    async fn lockdown_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>>;
    async fn rsd_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>>;
    async fn watch(&self) -> Result<BoxStream<'static, DeviceEvent>>;
    async fn call(&self, api: FacadeCall) -> Result<serde_json::Value>;

    /// A TCP connection to any port on the device's tunnel address (`ForwardTarget::TunnelPort`). Provided: the
    /// default answers `device.unsupported`, so connectors without a tunnel need nothing.
    async fn tunnel_port(&self, udid: &Udid, port: u16) -> Result<Box<dyn idevice::ReadWrite>> {
        let _ = udid;
        Err(device_error(codes::device::UNSUPPORTED, format!("this device backend cannot reach tunnel port {port}")))
    }

    /// A classic usbmux `Connect` to a device port (`ForwardTarget::UsbmuxPort`, iproxy-style). Provided, like
    /// [`Self::tunnel_port`].
    async fn usbmux_port(&self, udid: &Udid, port: u16) -> Result<Box<dyn idevice::ReadWrite>> {
        let _ = udid;
        Err(device_error(codes::device::UNSUPPORTED, format!("this device backend cannot reach usbmux port {port}")))
    }

    /// The backend kind for doctor and logs: `live`, `scripted`, `replay`, `mock` or `memory`.
    fn backend(&self) -> &'static str {
        "custom"
    }
}

/// The facade API catalog of [`ServiceConnector::call`]. Arguments and results are JSON (camelCase keys).
pub mod facade {
    use serde::{Deserialize, Serialize};

    /// `mux.list`: `{}` → `MuxRecord[]`, every device the mux reports.
    pub const MUX_LIST: &str = "mux.list";
    /// `mux.ping`: `{}` → `{endpoint, devices}`; doctor's `device.mux` (live backend only).
    pub const MUX_PING: &str = "mux.ping";
    /// `lockdown.values`: `{udid, keys: string[], domain?, session: bool}` → `{values: {key: json}}`. Keys the
    /// device does not answer are omitted; data values are lowercase hex strings.
    pub const LOCKDOWN_VALUES: &str = "lockdown.values";
    /// `lockdown.get_value`: `{udid?, key?, domain?}` → the value of `key`, or the domain's whole dictionary without
    /// `key` (QA §3.5.2's recorded call; russet-testkit's scripted connector answers it from the scenario). The
    /// device plane asks [`LOCKDOWN_VALUES`] first and falls back to this when a connector does not answer that.
    pub const LOCKDOWN_GET_VALUE: &str = "lockdown.get_value";
    /// `pairing.status`: `{udid}` → [`PairingStatus`].
    pub const PAIRING_STATUS: &str = "pairing.status";
    /// `pairing.pair`: `{udid, hostName?, timeoutMs?}` → `{status: paired | pending | denied | locked}`. Waits for
    /// the Trust dialog up to `timeoutMs` (default 120 s); on success the record is saved through the mux.
    pub const PAIRING_PAIR: &str = "pairing.pair";
    /// `tunnel.status`: `{udid}` → [`TunnelStatusRecord`].
    pub const TUNNEL_STATUS: &str = "tunnel.status";
    /// `tunnel.close`: `{udid}` → `{}`; the next RSD call opens a new generation.
    pub const TUNNEL_CLOSE: &str = "tunnel.close";
    /// `rsd.services`: `{udid}` → `{generation, services: {name: {port, features[]}}, properties}`.
    pub const RSD_SERVICES: &str = "rsd.services";
    /// `tunnel.kill`: `{udid}` → `{killed}`; stops the tunnel's TCP stack but leaves the tunnel in place, as if its
    /// stream had died, so the next RSD call exercises the rebuild path (tests and lab checks only).
    pub const TUNNEL_KILL: &str = "tunnel.kill";

    /// One `mux.list` entry.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct MuxRecord {
        pub udid: String,
        pub device_id: u32,
        /// `usb` or `wifi`.
        pub transport: String,
    }

    /// The `pairing.status` result.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PairingStatus {
        /// The mux holds a pair record for the device.
        pub record_found: bool,
        /// A lockdown session with that record succeeded.
        pub paired: bool,
        /// The `russetCode` of the failure, when `paired` is false.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub error: Option<String>,
    }

    /// The `tunnel.status` result.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct TunnelStatusRecord {
        /// `down`, `up` or `failed`.
        pub state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub generation: Option<u64>,
        /// RFC 3339 UTC.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub since: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub error: Option<russet_core::error::ErrorData>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub mss: Option<u64>,
    }
}

/// Whether a facade call failed only because the connector does not answer that API (`device.serviceNotImplemented`,
/// 3013): a connector that implements an older or smaller catalog. The device plane then uses a fallback where one
/// exists (`lockdown.get_value` for `lockdown.values`, a lockdown probe for `pairing.status`).
pub fn unanswered(e: &russet_core::Error) -> bool {
    e.code() == codes::device::SERVICE_NOT_IMPLEMENTED
}

/// The real connector: usbmuxd (or AMDS), lockdown, CoreDeviceProxy and the jktcp tunnel.
pub use live::LiveConnector;

static LIVE_CONNECTORS: AtomicUsize = AtomicUsize::new(0);

/// How many [`LiveConnector`]s this process constructed. Lane-D daemon tests assert 0 (ARCHITECTURE §3.3 item 7)
/// [D 83].
pub fn live_connectors_constructed() -> usize {
    LIVE_CONNECTORS.load(Ordering::SeqCst)
}

fn count_live_connector() {
    LIVE_CONNECTORS.fetch_add(1, Ordering::SeqCst);
}

impl LiveConnector {
    /// Constructs the live connector; increments [`live_connectors_constructed`] before anything else. Opens no
    /// socket: the mux is contacted on the first call (ARCHITECTURE §3.3 item 7).
    pub fn new(cfg: &DeviceConfig) -> Result<Self> {
        count_live_connector();
        live::LiveConnector::with_config(cfg)
    }
}

/// Converts a plist value to JSON for facade results: data becomes a lowercase hex string, dates RFC 3339.
pub fn plist_to_json(v: &plist::Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        plist::Value::Array(a) => J::Array(a.iter().map(plist_to_json).collect()),
        plist::Value::Dictionary(d) => J::Object(d.iter().map(|(k, v)| (k.clone(), plist_to_json(v))).collect()),
        plist::Value::Boolean(b) => J::Bool(*b),
        plist::Value::Data(d) => J::String(d.iter().map(|b| format!("{b:02x}")).collect()),
        plist::Value::Date(d) => J::String(
            time::OffsetDateTime::from(std::time::SystemTime::from(*d))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        ),
        plist::Value::Real(r) => serde_json::Number::from_f64(*r).map(J::Number).unwrap_or(J::Null),
        plist::Value::Integer(i) => match (i.as_signed(), i.as_unsigned()) {
            (Some(s), _) => J::from(s),
            (None, Some(u)) => J::from(u),
            _ => J::Null,
        },
        plist::Value::String(s) => J::String(s.clone()),
        plist::Value::Uid(u) => J::from(u.get()),
        _ => J::Null,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nothing_constructs_a_live_connector_by_itself() {
        assert_eq!(super::live_connectors_constructed(), 0);
    }

    #[test]
    fn plist_values_become_json() {
        let mut d = plist::Dictionary::new();
        d.insert("a".into(), plist::Value::Data(vec![0xde, 0xad]));
        d.insert("b".into(), 27.into());
        d.insert("c".into(), "27.0".into());
        d.insert("d".into(), true.into());
        let j = super::plist_to_json(&plist::Value::Dictionary(d));
        assert_eq!(j, serde_json::json!({"a": "dead", "b": 27, "c": "27.0", "d": true}));
    }
}
