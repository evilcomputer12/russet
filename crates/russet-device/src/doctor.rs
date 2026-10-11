// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet doctor` device checks (DEV-25 subset; device.md §10): the mux, pairing, Developer Mode, the DDI and the
//! lock hint, each with a remedy.
//!
//! Checks never construct a connector: they use the device plane the daemon already started
//! ([`DeviceManager::active`]). `usbmux.available` connects to the mux only through a live manager; otherwise it
//! stats the socket and reports `skip` [D 83]. The device checks report `skip` until the device plane started
//! (the first device call, or `doctor.run` with `udid`, which the doctor handler turns into a device call).

use std::sync::Arc;

use async_trait::async_trait;
use russet_doctor::{Category, Check, CheckCx, CheckResult, Requires, Status};

use crate::io::FacadeCall;
use crate::io::facade;
use crate::manager::{DeviceHandle, DeviceManager};
use crate::model::{DeviceSelector, Tri};

/// Every device check, in report order.
pub fn checks() -> Vec<Arc<dyn Check>> {
    vec![Arc::new(UsbmuxAvailable), Arc::new(Pairing), Arc::new(DeveloperMode), Arc::new(Ddi), Arc::new(LockHint)]
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

/// `usbmux.available`.
#[derive(Debug, Clone, Copy)]
pub struct UsbmuxAvailable;

#[async_trait]
impl Check for UsbmuxAvailable {
    fn id(&self) -> &'static str {
        "usbmux.available"
    }
    fn title(&self) -> &'static str {
        "usbmuxd (or the Apple Mobile Device Service) answers"
    }
    fn category(&self) -> Category {
        Category::Usbmux
    }
    fn requires(&self) -> Requires {
        Requires { daemon: true, ..Default::default() }
    }
    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        if let Some(m) = DeviceManager::active() {
            if m.connector().backend() == "live" {
                return match m.connector().call(FacadeCall::new(facade::MUX_PING, serde_json::json!({}))).await {
                    Ok(v) => CheckResult {
                        status: Status::Pass,
                        summary: format!(
                            "the mux answers at {} ({} device(s))",
                            v.get("endpoint").and_then(|e| e.as_str()).unwrap_or("?"),
                            v.get("devices").and_then(|d| d.as_u64()).unwrap_or(0)
                        ),
                        detail: None,
                        hint: None,
                    },
                    Err(e) => result(Status::Fail, e.message().to_owned(), e.hint()),
                };
            }
            return result(Status::Skip, format!("the {} device backend has no mux", m.connector().backend()), None);
        }
        // No live device plane: stat the socket, never connect [D 83].
        let endpoint = crate::mux::MuxEndpoint::from_setting("auto");
        match endpoint {
            #[cfg(unix)]
            Ok(crate::mux::MuxEndpoint::Unix(path)) => {
                let exists = std::fs::metadata(&path).is_ok();
                CheckResult {
                    status: Status::Skip,
                    summary: if exists {
                        format!("{} exists; not contacted until the live device plane starts", path.display())
                    } else {
                        format!("{} does not exist", path.display())
                    },
                    detail: None,
                    hint: (!exists).then(|| {
                        "Install and start usbmuxd (`systemctl start usbmuxd`, or plug the iPhone in to trigger it).".to_owned()
                    }),
                }
            }
            Ok(other) => result(
                Status::Skip,
                format!("{} is not contacted until the live device plane starts", other.describe()),
                None,
            ),
            Err(e) => result(Status::Fail, e.message().to_owned(), e.hint()),
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
        match h.summary().readiness.map(|r| r.locked).unwrap_or(Tri::Unknown) {
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
    fn ids_are_unique_and_in_the_device_categories() {
        let c = checks();
        let ids: std::collections::BTreeSet<&str> = c.iter().map(|c| c.id()).collect();
        assert_eq!(ids.len(), 5);
        assert!(ids.contains("usbmux.available"));
        assert!(c.iter().all(|c| matches!(c.category(), Category::Usbmux | Category::Device)));
    }
}
