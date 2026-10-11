// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Development isolation (ARCHITECTURE §3.3 item 7 [D 63, D 83]; PLAN W2 tests), with in-process daemons:
//! - a development daemon without `RUSSET_LANE=A` refuses the `live` backend: every device method answers
//!   `device.liveBackendRefused` (3019) and no `LiveConnector` is ever constructed;
//! - a second live daemon (`--allow-live-device`) answers 3019 with `details.holder {pid, home}` while the first
//!   holds the live-device lock, again without constructing a connector.
//!
//! The lock lives in each test's temporary home (debug builds honour `RUSSET_LIVE_DEVICE_LOCK`; these tests pass
//! the path in the options), never the real one, and nothing here sets `RUSSET_LANE=A`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use russet_core::error::codes;
use russet_ipc::CallOptions;
use russetd::DeviceBackend;
use russetd::ctx::{LiveRefusal, LiveStatus};
use serde_json::json;
use support::{Home, start};

#[tokio::test]
async fn a_lane_d_daemon_refuses_live_with_3019_and_constructs_no_connector() {
    let home = Home::new("iso-refused");
    let mut o = home.options();
    o.device_backend = DeviceBackend::Live;
    let (daemon, cli) = start(o).await;
    assert_eq!(daemon.ctx().devices.live_status(), LiveStatus::Refused(LiveRefusal::DevelopmentBuild));
    for (method, params) in [("device.list", json!({})), ("app.list", json!({})), ("logs.tail", json!({}))] {
        let e = cli.call_raw(method, params, CallOptions::default()).await.expect_err("refused");
        assert_eq!(e.to_error().code(), codes::device::LIVE_BACKEND_REFUSED, "{method}");
        let details = e.to_error().details().cloned().expect("details");
        assert_eq!(details["reason"], "development-build");
    }
    let e = daemon.ctx().devices.manager().await.expect_err("the slot refuses too");
    assert_eq!(e.code(), codes::device::LIVE_BACKEND_REFUSED);
    assert!(!home.dir.path().join("live-device.lock").exists(), "a refused daemon never takes the lock");
    assert_eq!(russet_device::io::live_connectors_constructed(), 0);
    daemon.stop().await.expect("stop");
}

#[tokio::test]
async fn a_second_live_daemon_gets_3019_with_the_holder() {
    let shared = Home::new("iso-lock");
    let lock = shared.dir.path().join("russet-live-device.lock");
    let first_home = Home::new("iso-first");
    let mut first = first_home.options();
    first.device_backend = DeviceBackend::Live;
    first.allow_live_device = true;
    first.live_device_lock = lock.clone();
    let (d1, _c1) = start(first).await;
    assert_eq!(d1.ctx().devices.live_status(), LiveStatus::Held);

    let second_home = Home::new("iso-second");
    let mut second = second_home.options();
    second.device_backend = DeviceBackend::Live;
    second.allow_live_device = true;
    second.live_device_lock = lock.clone();
    let (d2, c2) = start(second).await;
    let e = c2.call_raw("device.list", json!({}), CallOptions::default()).await.expect_err("refused").to_error();
    assert_eq!(e.code(), codes::device::LIVE_BACKEND_REFUSED);
    let holder = e.details().and_then(|d| d.get("holder")).cloned().expect("details.holder");
    assert_eq!(holder["pid"], std::process::id());
    assert_eq!(holder["home"], first_home.dir.path().display().to_string());
    assert_eq!(russet_device::io::live_connectors_constructed(), 0);

    // The holder stops: the lock is free for the next live daemon.
    d2.stop().await.expect("stop");
    d1.stop().await.expect("stop");
    let third_home = Home::new("iso-third");
    let mut third = third_home.options();
    third.device_backend = DeviceBackend::Live;
    third.allow_live_device = true;
    third.live_device_lock = lock;
    let (d3, _c3) = start(third).await;
    assert_eq!(d3.ctx().devices.live_status(), LiveStatus::Held);
    d3.stop().await.expect("stop");
}

#[tokio::test]
async fn other_backends_reach_the_handlers() {
    let home = Home::new("iso-none");
    let (daemon, cli) = start(home.options()).await;
    // RUSSET_DEVICE_BACKEND=none passes the guard and reaches W4's handler, which has no device plane.
    let e = cli.call_raw("device.list", json!({}), CallOptions::default()).await.expect_err("no plane").to_error();
    assert_eq!(e.code(), codes::device::USBMUX_UNAVAILABLE);
    let e = daemon.ctx().devices.manager().await.expect_err("no device plane");
    assert_eq!(e.code(), codes::device::USBMUX_UNAVAILABLE);
    assert_eq!(russet_device::io::live_connectors_constructed(), 0);
    daemon.stop().await.expect("stop");
}
