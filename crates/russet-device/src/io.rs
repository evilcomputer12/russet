// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The device I/O seam (testability contract T5, ARCHITECTURE §5.3 `io`): plaintext service streams after TLS
//! (lockdown) or inside the tunnel (RSD). Scripted, replay and recording connectors live in russet-testkit.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream::BoxStream;
use russet_core::Result;

use crate::manager::{DeviceConfig, DeviceEvent};
use crate::model::Udid;

/// A facade-level call, for recording and replay at the API level (QA §3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacadeCall {
    pub api: String,
    pub args: serde_json::Value,
}

/// Opens device service streams and watches attach/detach.
#[async_trait]
pub trait ServiceConnector: Send + Sync + 'static {
    async fn lockdown_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>>;
    async fn rsd_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>>;
    async fn watch(&self) -> Result<BoxStream<'static, DeviceEvent>>;
    async fn call(&self, api: FacadeCall) -> Result<serde_json::Value>;
}

/// The real connector: usbmuxd (or AMDS), lockdown, CoreDeviceProxy and the jktcp tunnel.
#[derive(Debug)]
pub struct LiveConnector {
    _f0: (),
}

static LIVE_CONNECTORS: AtomicUsize = AtomicUsize::new(0);

/// How many [`LiveConnector`]s this process constructed. Lane-D daemon tests assert 0 (ARCHITECTURE §3.3 item 7)
/// [D 83].
pub fn live_connectors_constructed() -> usize {
    LIVE_CONNECTORS.load(Ordering::SeqCst)
}

impl LiveConnector {
    /// Constructs the live connector; increments [`live_connectors_constructed`] before anything else.
    pub fn new(cfg: &DeviceConfig) -> Result<Self> {
        LIVE_CONNECTORS.fetch_add(1, Ordering::SeqCst);
        todo!("W4 (DEV-01, DEV-04)")
    }
}

#[async_trait]
impl ServiceConnector for LiveConnector {
    async fn lockdown_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        todo!("W4 (DEV-02)")
    }
    async fn rsd_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        todo!("W4 (DEV-04)")
    }
    async fn watch(&self) -> Result<BoxStream<'static, DeviceEvent>> {
        todo!("W4 (DEV-02)")
    }
    async fn call(&self, api: FacadeCall) -> Result<serde_json::Value> {
        todo!("W4")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nothing_constructs_a_live_connector_by_itself() {
        assert_eq!(super::live_connectors_constructed(), 0);
    }
}
