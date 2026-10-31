// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet doctor` device checks (DEV-25 subset; device.md §10): the mux flavor, pairing, Developer Mode, the DDI
//! and the lock hint, each with a remedy.
//!
//! Checks never construct a connector and never connect to anything on their own: they use the device plane the
//! daemon already started ([`DeviceManager::active`]) and report `skip` until it did (the first device call, or
//! `doctor.run` with `udid`, which the doctor handler turns into a device call) [D 83]. Whether usbmuxd itself
//! answers is W3's host check `usbmux.available` (CORE-12); `device.mux` reports which mux the running device plane
//! talks to and what it sees.

use std::sync::Arc;

use async_trait::async_trait;
use russet_doctor::{Category, Check, CheckCx, CheckResult, Requires, Status};

use crate::io::FacadeCall;
use crate::io::facade;
use crate::manager::{DeviceHandle, DeviceManager};
use crate::model::{DeviceSelector, Tri};
use crate::mux::MuxEndpoint;

/// Every device check, in report order.
pub fn checks() -> Vec<Arc<dyn Check>> {
    vec![Arc::new(Mux), Arc::new(Pairing), Arc::new(DeveloperMode), Arc::new(Ddi), Arc::new(LockHint)]
}

fn result(status: Status, summary: impl Into<String>, hint: Option<&str>) -> CheckResult {
    CheckResult { status, summary: summary.into(), detail: None, hint: hint.map(str::to_owned) }
}

fn not_started() -> CheckResult {
    result(
        Status::Skip,
        "the device plane has not started in this daemon",
        Some("Run `russet device list` (or `russet doctor --device <udid>`) to start it."),
    )
}

/// The device the check runs against: `cx.udid` (UDID or alias), else the only attached device.
fn device(cx: &CheckCx) -> Result<DeviceHandle, CheckResult> {
    let Some(m) = DeviceManager::active() else { return Err(not_started()) };
    m.get(&DeviceSelector::parse(cx.udid.as_deref()))
        .map_err(|e| result(Status::Skip, format!("no device to check: {}", e.message()), e.hint()))
}

/// The mux flavor of an endpoint (device.md §10): usbmuxd on a Unix socket, the Apple Mobile Device Service on
/// loopback port 27015, netmuxd on any other TCP address.
pub fn mux_flavor(endpoint: &MuxEndpoint) -> &'static str {
    match endpoint {
        #[cfg(unix)]
        MuxEndpoint::Unix(_) => "usbmuxd",
        MuxEndpoint::Tcp(a) if a.ip().is_loopback() && a.port() == 27015 => "Apple Mobile Device Service",
        MuxEndpoint::Tcp(_) => "netmuxd",
    }
}

/// `device.mux`: the mux flavor and endpoint the running device plane uses, and how many devices it reports.
#[derive(Debug, Clone, Copy)]
pub struct Mux;

#[async_trait]
impl Check for Mux {
    fn id(&self) -> &'static str {
        "device.mux"
    }
    fn title(&self) -> &'static str {
        "The device plane reaches its mux (usbmuxd, AMDS or netmuxd)"
    }
    fn category(&self) -> Category {
        Category::Usbmux
    }
    fn requires(&self) -> Requires {
        Requires { daemon: true, ..Default::default() }
    }
    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        let Some(m) = DeviceManager::active() else { return not_started() };
        let backend = m.connector().backend();
        if backend != "live" {
            return result(Status::Skip, format!("the {backend} device backend has no mux"), None);
        }
        let flavor = MuxEndpoint::from_setting(&m.config().mux).map(|e| mux_flavor(&e)).unwrap_or("mux");
        match m.connector().call(FacadeCall::new(facade::MUX_PING, serde_json::json!({}))).await {
            Ok(v) => {
                let devices = v.get("devices").and_then(|d| d.as_u64()).unwrap_or(0);
                CheckResult {
                    status: if devices == 0 { Status::Warn } else { Status::Pass },
                    summary: format!(
                        "{flavor} at {} reports {devices} device(s)",
                        v.get("endpoint").and_then(|e| e.as_str()).unwrap_or("?")
                    ),
                    detail: None,
                    hint: (devices == 0)
                        .then(|| "Connect the iPhone with a USB cable, unlock it and tap Trust if it asks.".to_owned()),
                }
            }
            Err(e) => result(Status::Fail, format!("{flavor}: {}", e.message()), e.hint()),
        }
    }
}

/// `device.pairing`.
#[derive(Debug, Clone, Copy)]
pub struct Pairing;

#[async_trait]
impl Check for Pairing {
    fn id(&self) -> &'static str {
        "device.pairing"
    }
    fn title(&self) -> &'static str {
        "The iPhone trusts this computer"
    }
    fn category(&self) -> Category {
        Category::Device
    }
    fn requires(&self) -> Requires {
        Requires { daemon: true, device: true, ..Default::default() }
    }
    async fn run(&self, cx: &CheckCx) -> CheckResult {
        let h = match device(cx) {
            Ok(h) => h,
            Err(r) => return r,
        };
        match h.pairing_status().await {
            Ok(s) if s.paired => result(Status::Pass, "paired; a lockdown session works", None),
            Ok(s) if !s.record_found => result(
                Status::Fail,
                "no pair record for this iPhone",
                Some("Run `russet device pair`, then unlock the iPhone and tap Trust."),
            ),
            Ok(s) => result(
                Status::Fail,
                format!("the pair record does not work ({})", s.error.unwrap_or_default()),
                Some("Run `russet device pair` again; if it keeps failing, reset Location & Privacy on the iPhone."),
            ),
            Err(e) => result(Status::Fail, e.message().to_owned(), e.hint()),
        }
    }
}

/// `device.developerMode`.
#[derive(Debug, Clone, Copy)]
pub struct DeveloperMode;

#[async_trait]
impl Check for DeveloperMode {
    fn id(&self) -> &'static str {
        "device.developerMode"
    }
    fn title(&self) -> &'static str {
        "Developer Mode is on"
    }
    fn category(&self) -> Category {
        Category::Device
    }
    fn requires(&self) -> Requires {
        Requires { daemon: true, device: true, ..Default::default() }
    }
    async fn run(&self, cx: &CheckCx) -> CheckResult {
        let h = match device(cx) {
            Ok(h) => h,
            Err(r) => return r,
        };
        match h.developer_mode().await {
            Ok(Tri::Yes) => result(Status::Pass, "Developer Mode is on", None),
            Ok(Tri::No) => result(
                Status::Fail,
                "Developer Mode is off",
                crate::error::hint_for(russet_core::error::codes::device::DEVELOPER_MODE_DISABLED),
            ),
            Ok(Tri::Unknown) => result(Status::Warn, "Developer Mode could not be read", None),
            Err(e) => result(Status::Fail, e.message().to_owned(), e.hint()),
        }
    }
}

/// `device.ddi`.
#[derive(Debug, Clone, Copy)]
pub struct Ddi;

#[async_trait]
impl Check for Ddi {
    fn id(&self) -> &'static str {
        "device.ddi"
    }
    fn title(&self) -> &'static str {
        "A Developer Disk Image is mounted"
    }
    fn category(&self) -> Category {
        Category::Device
    }
    fn requires(&self) -> Requires {
        Requires { daemon: true, device: true, ..Default::default() }
    }
    async fn run(&self, cx: &CheckCx) -> CheckResult {
        let h = match device(cx) {
            Ok(h) => h,
            Err(r) => return r,
        };
        match h.ddi_status().await {
            Ok(s) if s.mounted => CheckResult {
                status: Status::Pass,
                summary: format!(
                    "mounted: {} {} (source {})",
                    s.product_build.as_deref().unwrap_or("?"),
                    s.version.as_deref().unwrap_or(""),
                    crate::wire::origin_name(s.source)
                ),
                detail: None,
                hint: None,
            },
            Ok(_) => result(
                Status::Warn,
                "no Developer Disk Image is mounted",
                crate::error::hint_for(russet_core::error::codes::device::DDI_NOT_MOUNTED),
            ),
            Err(e) => result(Status::Fail, e.message().to_owned(), e.hint()),
        }
    }
}

/// `device.lockHint`: iOS 27 cannot be asked whether it is locked (`getlockstate` is not implemented, device.md
/// F9), so this reports what the last operations saw.
#[derive(Debug, Clone, Copy)]
pub struct LockHint;

#[async_trait]
impl Check for LockHint {
    fn id(&self) -> &'static str {
        "device.lockHint"
    }
    fn title(&self) -> &'static str {
        "The iPhone is unlocked"
    }
    fn category(&self) -> Category {
        Category::Device
    }
    fn requires(&self) -> Requires {
        Requires { daemon: true, device: true, ..Default::default() }
    }
    async fn run(&self, cx: &CheckCx) -> CheckResult {
        let h = match device(cx) {
            Ok(h) => h,
            Err(r) => return r,
        };
        match h.lock_hint() {
            Tri::Yes => result(
                Status::Warn,
                "the last operation found the iPhone locked",
                crate::error::hint_for(russet_core::error::codes::device::LOCKED),
            ),
            Tri::No => result(Status::Pass, "the last launch found the iPhone unlocked", None),
            Tri::Unknown => result(
                Status::Pass,
                "no lock reported (iOS cannot be asked directly; keep the iPhone unlocked while installing and launching)",
                None,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mux_flavors_follow_the_endpoint() {
        assert_eq!(mux_flavor(&MuxEndpoint::from_setting("amds").expect("amds")), "Apple Mobile Device Service");
        assert_eq!(mux_flavor(&MuxEndpoint::from_setting("netmuxd:127.0.0.1:27016").expect("netmuxd")), "netmuxd");
        #[cfg(unix)]
        assert_eq!(mux_flavor(&MuxEndpoint::from_setting("usbmuxd:/run/usbmuxd").expect("usbmuxd")), "usbmuxd");
    }

    #[test]
    fn ids_are_unique_and_in_the_device_categories() {
        let c = checks();
        let ids: std::collections::BTreeSet<&str> = c.iter().map(|c| c.id()).collect();
        assert_eq!(ids.len(), 5);
        // `usbmux.available` is W3's host check (CORE-12); the device plane's mux check has its own id.
        assert!(ids.contains("device.mux"));
        assert!(!ids.contains("usbmux.available"));
        assert!(c.iter().all(|c| matches!(c.category(), Category::Usbmux | Category::Device)));
    }
}
