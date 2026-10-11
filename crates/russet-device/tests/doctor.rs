// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The device doctor checks (DEV-25 subset) against the device plane that is running. They find it through
//! `DeviceManager::active()`, which is process-wide, so this test binary starts exactly one manager.

mod support;

use std::sync::Arc;

use russet_device::io::ServiceConnector;
use russet_device::{DeviceConfig, DeviceManager};
use russet_doctor::{CheckCx, Status};
use support::{FakeConnector, FakeDevice, UDID_A};

#[tokio::test]
async fn doctor_checks_use_the_running_device_plane() {
    // Before the device plane starts, the device checks skip instead of connecting to anything.
    let checks = russet_device::checks();
    let pairing = checks.iter().find(|c| c.id() == "device.pairing").expect("pairing check");
    assert_eq!(pairing.run(&CheckCx::default()).await.status, Status::Skip);

    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = DeviceConfig {
        state_dir: tmp.path().join("state"),
        runtime_dir: tmp.path().join("runtime"),
        ..Default::default()
    };
    let conn = FakeConnector::new(vec![FakeDevice::iphone12(UDID_A)]);
    let m = DeviceManager::start(cfg, conn as Arc<dyn ServiceConnector>).await.expect("start");
    let r = pairing.run(&CheckCx::default()).await;
    assert_eq!(r.status, Status::Pass, "{r:?}");
    let lock = checks.iter().find(|c| c.id() == "device.lockHint").expect("lock check");
    assert_eq!(lock.run(&CheckCx::default()).await.status, Status::Pass);
    let usbmux = checks.iter().find(|c| c.id() == "usbmux.available").expect("usbmux check");
    // A non-live backend has no mux to contact [D 83].
    assert_eq!(usbmux.run(&CheckCx::default()).await.status, Status::Skip);
    drop(m);
}
