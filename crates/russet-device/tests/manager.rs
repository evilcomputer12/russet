// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The device manager over an in-memory connector: lazy identity [D 63], attach/detach events, selectors and
//! aliases, reservations [D 68], forwards (ARCHITECTURE §10.4), the rpc layer and the doctor checks.

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use russet_core::error::codes;
use russet_device::io::ServiceConnector;
use russet_device::model::{DeviceState, Udid};
use russet_device::rpc::{self, Caller};
use russet_device::{DeviceConfig, DeviceEvent, DeviceManager, DeviceSelector, ForwardTarget};
use support::{FakeConnector, FakeDevice, UDID_A, UDID_B};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn config(dir: &std::path::Path) -> DeviceConfig {
    DeviceConfig {
        state_dir: dir.join("state"),
        runtime_dir: dir.join("runtime"),
        data_dir: dir.join("data"),
        config_dir: dir.join("config"),
        ..Default::default()
    }
}

async fn start(devices: Vec<FakeDevice>) -> (tempfile::TempDir, Arc<FakeConnector>, Arc<DeviceManager>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let conn = FakeConnector::new(devices);
    let m = DeviceManager::start(config(tmp.path()), conn.clone() as Arc<dyn ServiceConnector>).await.expect("start");
    (tmp, conn, m)
}

async fn next_event(rx: &mut tokio::sync::broadcast::Receiver<DeviceEvent>) -> DeviceEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("an event in time").expect("event")
}

#[tokio::test]
async fn identity_is_read_lazily_on_the_first_list() {
    let (_tmp, conn, m) = start(vec![FakeDevice::iphone12(UDID_A)]).await;
    // Starting the manager only listed the mux; nobody asked yet, so no lockdown value was read.
    assert_eq!(conn.value_calls.load(Ordering::SeqCst), 0);
    let cached = m.devices();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].state, DeviceState::Attached);
    assert!(cached[0].identity.is_none());

    let list = rpc::device_list(&m).await.expect("device.list");
    assert_eq!(list.devices.len(), 1);
    let d = &list.devices[0];
    assert_eq!((d.product_type.as_str(), d.os_version.as_str()), ("iPhone13,2", "27.0"));
    assert_eq!(d.marketing_name.as_deref(), Some("iPhone 12"));
    let json = serde_json::to_value(d).expect("json");
    assert_eq!(json["state"], "ready");
    assert_eq!(json["codeSigningMonitor"], "txm");
    let calls = conn.value_calls.load(Ordering::SeqCst);
    // A second list answers from the cache.
    rpc::device_list(&m).await.expect("device.list");
    assert_eq!(conn.value_calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn unpaired_devices_need_trust_and_keep_their_public_identity() {
    let mut d = FakeDevice::iphone12(UDID_A);
    d.paired = false;
    let (_tmp, _conn, m) = start(vec![d]).await;
    let list = rpc::device_list(&m).await.expect("device.list");
    let json = serde_json::to_value(&list.devices[0]).expect("json");
    assert_eq!(json["state"], "needs-trust");
    assert_eq!(json["productType"], "iPhone13,2");
    let h = m.get(&DeviceSelector::Only).expect("the only device");
    let r = h.readiness(true).await.expect("readiness");
    assert!(!r.paired);
}

#[tokio::test]
async fn attach_and_detach_events_reach_watchers() {
    let (_tmp, conn, m) = start(vec![FakeDevice::iphone12(UDID_A)]).await;
    let mut rx = m.events();
    conn.attach(FakeDevice::iphone12(UDID_B));
    match next_event(&mut rx).await {
        DeviceEvent::Added(s) => assert_eq!(s.udid.as_str(), UDID_B),
        other => panic!("expected Added, got {other:?}"),
    }
    // A watcher counts as a client asking: the identity is read in the background and announced.
    let mut ready = false;
    for _ in 0..5 {
        if let DeviceEvent::Changed(s) = next_event(&mut rx).await
            && s.udid.as_str() == UDID_B
            && s.identity.is_some()
        {
            ready = true;
            break;
        }
    }
    assert!(ready, "the new device's identity was announced");
    assert_eq!(m.devices().len(), 2);

    // Two devices: the selector must name one.
    let err = m.get(&DeviceSelector::Only).expect_err("ambiguous");
    assert_eq!(err.code(), codes::core::INVALID_ARGUMENT);

    conn.detach(UDID_B);
    loop {
        if let DeviceEvent::Removed(u) = next_event(&mut rx).await {
            assert_eq!(u.as_str(), UDID_B);
            break;
        }
    }
    assert_eq!(m.devices().len(), 1);
    let err = m.get(&DeviceSelector::Udid(Udid(UDID_B.into()))).expect_err("gone");
    assert_eq!(err.code(), codes::device::NOT_FOUND);
}

#[tokio::test]
async fn aliases_resolve_and_are_written_to_config() {
    let (tmp, _conn, m) = start(vec![FakeDevice::iphone12(UDID_A)]).await;
    let path = m.set_alias("iphone12", &Udid(UDID_A.into())).expect("alias");
    assert!(std::fs::read_to_string(&path).expect("config").contains("iphone12"));
    assert!(path.starts_with(tmp.path()));
    let h = m.get(&DeviceSelector::parse(Some("iphone12"))).expect("by alias");
    assert_eq!(h.udid().as_str(), UDID_A);
    let err = m.get(&DeviceSelector::parse(Some("nope"))).expect_err("unknown alias");
    assert_eq!(err.code(), codes::device::NOT_FOUND);
    assert!(err.hint().is_some());
}

#[tokio::test]
async fn reservations_exclude_other_principals_except_sharers() {
    let (_tmp, _conn, m) = start(vec![FakeDevice::iphone12(UDID_A)]).await;
    let owner = Caller { conn: "c-1".into(), shared_lease: None };
    let r = rpc::device_reserve(&m, &owner, Default::default()).expect("reserve");
    // The owner may reserve again (idempotent); another connection may not.
    assert_eq!(rpc::device_reserve(&m, &owner, Default::default()).expect("again").lease_id, r.lease_id);
    let other = Caller { conn: "c-2".into(), shared_lease: None };
    let err = rpc::handle_for(&m, None, &other).expect_err("reserved");
    assert_eq!(err.code(), codes::device::RESERVED);
    assert_eq!(err.code().number(), 3018);
    // A connection that presents the lease as sharedLease is a principal too [D 68].
    let sharer = Caller { conn: "c-3".into(), shared_lease: Some(r.lease_id.clone()) };
    rpc::handle_for(&m, None, &sharer).expect("sharer");
    assert!(m.release(&r.lease_id));
    rpc::handle_for(&m, None, &other).expect("released");
}

#[tokio::test]
async fn forwards_relay_bytes_and_close_by_lease() {
    let (_tmp, _conn, m) = start(vec![FakeDevice::iphone12(UDID_A)]).await;
    let caller = Caller::default();
    let params = russet_protocol::forward::ForwardOpenParams {
        udid: None,
        target: russet_protocol::forward::ForwardTargetParam {
            service: Some("test.echo".into()),
            ..Default::default()
        },
        bind: Some(russet_protocol::forward::ForwardBindParam { tcp: Some("127.0.0.1:0".into()), unix: None }),
    };
    let (opened, handle) = rpc::forward_open(&m, &caller, params).await.expect("forward.open");
    assert!(opened.url.starts_with("connect://127.0.0.1:"));
    let mut s = tokio::net::TcpStream::connect(opened.local.as_str()).await.expect("connect");
    s.write_all(b"ping").await.expect("write");
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut buf)).await.expect("in time").expect("read");
    assert_eq!(&buf, b"ping");
    let list = rpc::forward_list(&m);
    assert_eq!(list.forwards.len(), 1);
    assert_eq!(list.forwards[0].connections, 1);
    assert_eq!(list.forwards[0].target.service.as_deref(), Some("test.echo"));

    rpc::forward_close(&m, russet_protocol::forward::ForwardCloseParams { lease_id: opened.lease_id.clone() })
        .expect("forward.close");
    tokio::time::timeout(Duration::from_secs(5), handle.closed()).await.expect("closed in time");
    assert!(rpc::forward_list(&m).forwards.is_empty());
    let again = rpc::forward_close(&m, russet_protocol::forward::ForwardCloseParams { lease_id: opened.lease_id });
    assert_eq!(again.expect_err("unknown lease").code(), codes::core::NOT_FOUND);
}

#[cfg(unix)]
#[tokio::test]
async fn unix_forwards_are_private_and_named_after_the_lease() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, _conn, m) = start(vec![FakeDevice::iphone12(UDID_A)]).await;
    let h = m.get(&DeviceSelector::Only).expect("device");
    let f = russet_device::debug::open_forward_with(&h, ForwardTarget::TunnelPort(1234), None).await.expect("forward");
    let path = match &f.local {
        russet_device::LocalEndpoint::Unix(p) => p.clone(),
        other => panic!("expected a Unix socket, got {other:?}"),
    };
    assert!(
        path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("fwd-lse_") && n.ends_with(".sock"))
    );
    let mode = std::fs::metadata(&path).expect("socket").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let mut s = tokio::net::UnixStream::connect(&path).await.expect("connect");
    s.write_all(b"pong").await.expect("write");
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut buf)).await.expect("in time").expect("read");
    assert_eq!(&buf, b"pong");
    drop(f);
    // The socket file goes with the forward.
    for _ in 0..50 {
        if !path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!path.exists());
}
