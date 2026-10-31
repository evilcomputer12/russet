// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The device doctor checks (DEV-25 subset) against the device plane that is running. They find it through
//! `DeviceManager::active()`, which is process-wide, so this test binary starts exactly one manager.

mod support;

use std::sync::Arc;

use russet_core::error::codes;
use russet_device::io::ServiceConnector;
use russet_device::model::Tri;
use russet_device::{DeviceConfig, DeviceManager, DeviceSelector, LaunchRequest};
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
    let m = DeviceManager::start(cfg, conn.clone() as Arc<dyn ServiceConnector>).await.expect("start");
    let r = pairing.run(&CheckCx::default()).await;
    assert_eq!(r.status, Status::Pass, "{r:?}");
    let lock = checks.iter().find(|c| c.id() == "device.lockHint").expect("lock check");
    assert_eq!(lock.run(&CheckCx::default()).await.status, Status::Pass);
    let mux = checks.iter().find(|c| c.id() == "device.mux").expect("mux check");
    // A non-live backend has no mux to contact [D 83]; `usbmux.available` is W3's host check, not this crate's.
    assert_eq!(mux.run(&CheckCx::default()).await.status, Status::Skip);
    assert!(checks.iter().all(|c| c.id() != "usbmux.available"));

    // A launch that finds the iPhone locked sets the hint: the check warns at once (no readiness read needed), and
    // the device's summary carries it, so `device.list` and `device.changed` show it too.
    let h = m.get(&DeviceSelector::Only).expect("device");
    assert!(h.summary().readiness.is_none(), "nobody asked for readiness yet");
    conn.locked.store(true, std::sync::atomic::Ordering::SeqCst);
    let err = h
        .launch(LaunchRequest { bundle_id: "dev.russet.test.probe".into(), console: true, ..Default::default() })
        .await
        .expect_err("locked");
    assert_eq!(err.code(), codes::device::LOCKED);
    assert_eq!(h.lock_hint(), Tri::Yes);
    let r = lock.run(&CheckCx::default()).await;
    assert_eq!(r.status, Status::Warn, "{r:?}");
    assert!(r.hint.is_some());
    let mut seen = None;
    for _ in 0..100 {
        if let Some(r) = h.summary().readiness {
            seen = Some(r);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let readiness = seen.expect("the lock report read readiness once");
    assert_eq!(readiness.locked, Tri::Yes);
    assert!(readiness.paired);
    let wire = serde_json::to_value(russet_device::wire::summary(&h.summary())).expect("json");
    assert_eq!(wire["readiness"]["locked"], "yes");
    drop(m);
}
