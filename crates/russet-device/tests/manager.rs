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
use russet_device::{DeviceConfig, DeviceEvent, DeviceManager, DeviceSelector};
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
    let caller = Caller { conn: "c-1".into(), shared_lease: None };
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

    // Only the connection that opened it closes it (ARCHITECTURE §6.3), not another one, not even a sharer.
    let close =
        |lease: &russet_core::ids::LeaseId| russet_protocol::forward::ForwardCloseParams { lease_id: lease.clone() };
    let other = Caller { conn: "c-2".into(), shared_lease: None };
    let err = rpc::forward_close(&m, &other, close(&opened.lease_id)).expect_err("not the owner");
    assert_eq!(err.code(), codes::perm::DENIED);
    assert_eq!(rpc::forward_list(&m).forwards.len(), 1, "still open");
    rpc::forward_close(&m, &caller, close(&opened.lease_id)).expect("forward.close");
    tokio::time::timeout(Duration::from_secs(5), handle.closed()).await.expect("closed in time");
    assert!(rpc::forward_list(&m).forwards.is_empty());
    let again = rpc::forward_close(&m, &caller, close(&opened.lease_id));
    assert_eq!(again.expect_err("unknown lease").code(), codes::core::NOT_FOUND);
}

fn echo_target() -> russet_device::ForwardTarget {
    russet_device::ForwardTarget::RsdService("test.echo".into())
}

async fn echo_once(addr: &str) -> std::io::Result<()> {
    let mut s = tokio::net::TcpStream::connect(addr).await?;
    s.write_all(b"ping").await?;
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut buf)).await??;
    assert_eq!(&buf, b"ping");
    Ok(())
}

/// device.md §14: gdb-remote over TCP accepts one client and closes when nobody connects within the deadline.
#[tokio::test]
async fn accept_once_forwards_take_one_client_and_deadlines_close_idle_ones() {
    use russet_device::{AcceptPolicy, BindSpec, ForwardOptions};
    let (_tmp, _conn, m) = start(vec![FakeDevice::iphone12(UDID_A)]).await;
    let h = m.get(&DeviceSelector::Only).expect("device");
    let tcp = || Some(BindSpec::Tcp("127.0.0.1:0".parse().expect("addr")));

    // The default policy: a debugserver service over TCP accepts once, with the 60 s deadline.
    let debugproxy = russet_device::ForwardTarget::RsdService("com.apple.internal.dt.remote.debugproxy".into());
    let f =
        russet_device::debug::open_forward_opts(&h, debugproxy, ForwardOptions { bind: tcp(), ..Default::default() })
            .await
            .expect("forward");
    let listed = m.forwards().into_iter().find(|i| i.lease == f.id).expect("listed");
    assert!(listed.policy.accept_once);
    assert_eq!(listed.policy.first_connect_deadline, Some(Duration::from_secs(60)));
    drop(f);

    // Accept-once: the first client is relayed; the listener is gone for the next; the forward ends with the first.
    let once = AcceptPolicy { accept_once: true, first_connect_deadline: Some(Duration::from_secs(30)) };
    let f = russet_device::debug::open_forward_opts(
        &h,
        echo_target(),
        ForwardOptions { bind: tcp(), policy: Some(once), ..Default::default() },
    )
    .await
    .expect("forward");
    let addr = f.local.display();
    let mut first = tokio::net::TcpStream::connect(addr.as_str()).await.expect("first client");
    first.write_all(b"ping").await.expect("write");
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), first.read_exact(&mut buf)).await.expect("in time").expect("read");
    let mut refused = false;
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr.as_str()).await.is_err() {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(refused, "a second client is refused once the first one was accepted");
    drop(first);
    tokio::time::timeout(Duration::from_secs(5), f.closed()).await.expect("the forward ends with its one client");
    assert!(m.forwards().is_empty());

    // The deadline: nobody connects, the forward closes by itself.
    let quick = AcceptPolicy { accept_once: true, first_connect_deadline: Some(Duration::from_millis(200)) };
    let f = russet_device::debug::open_forward_opts(
        &h,
        echo_target(),
        ForwardOptions { bind: tcp(), policy: Some(quick), ..Default::default() },
    )
    .await
    .expect("forward");
    tokio::time::timeout(Duration::from_secs(5), f.closed()).await.expect("closed by the deadline");
    assert!(m.forwards().is_empty());
    assert!(echo_once(&f.local.display()).await.is_err(), "nothing listens any more");

    // Without limits, several clients share a forward (Flutter's VM service needs that).
    let f = russet_device::debug::open_forward_opts(
        &h,
        echo_target(),
        ForwardOptions { bind: tcp(), policy: Some(AcceptPolicy::OPEN), ..Default::default() },
    )
    .await
    .expect("forward");
    echo_once(&f.local.display()).await.expect("first");
    echo_once(&f.local.display()).await.expect("second");
}

/// A caller-chosen Unix path never destroys a file or loosens a directory (the review's probe, kept as a test).
#[cfg(unix)]
#[tokio::test]
async fn caller_socket_paths_never_replace_files_or_chmod_directories() {
    use russet_device::{BindSpec, ForwardOptions};
    use std::os::unix::fs::PermissionsExt;
    let (tmp, _conn, m) = start(vec![FakeDevice::iphone12(UDID_A)]).await;
    let h = m.get(&DeviceSelector::Only).expect("device");
    let shared = tmp.path().join("shared");
    std::fs::create_dir(&shared).expect("mkdir");
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let notes = shared.join("notes.txt");
    std::fs::write(&notes, "keep me").expect("seed");
    let open = |p: std::path::PathBuf| {
        let h = h.clone();
        async move {
            russet_device::debug::open_forward_opts(
                &h,
                echo_target(),
                ForwardOptions { bind: Some(BindSpec::Unix(p)), ..Default::default() },
            )
            .await
        }
    };
    let mode = |p: &std::path::Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;

    // An existing regular file is refused and left alone; the directory keeps its mode.
    let err = open(notes.clone()).await.expect_err("a regular file is in the way");
    assert_eq!(err.code(), codes::core::INVALID_ARGUMENT);
    assert_eq!(std::fs::read_to_string(&notes).expect("intact"), "keep me");
    assert_eq!(mode(&shared), 0o755);

    // Relative paths and missing directories are refused; nothing is created.
    assert!(open("relative.sock".into()).await.is_err());
    let missing = shared.join("no-such-dir").join("x.sock");
    assert_eq!(open(missing.clone()).await.expect_err("missing dir").code(), codes::core::INVALID_ARGUMENT);
    assert!(!missing.parent().expect("parent").exists());

    // A fresh path: a 0600 socket in the caller's directory, whose mode stays 0755.
    let sock = shared.join("dbg.sock");
    let f = open(sock.clone()).await.expect("a fresh path");
    assert_eq!(mode(&sock), 0o600);
    assert_eq!(mode(&shared), 0o755);
    let mut s = tokio::net::UnixStream::connect(&sock).await.expect("connect");
    s.write_all(b"ping").await.expect("write");
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut buf)).await.expect("in time").expect("read");
    assert_eq!(&buf, b"ping");
    // A second forward on the same path is refused (it is live).
    assert_eq!(open(sock.clone()).await.expect_err("in use").code(), codes::core::INVALID_ARGUMENT);
    drop(s);
    drop(f);
    for _ in 0..50 {
        if !sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!sock.exists(), "the socket Russet bound goes with the forward");

    // A stale socket (nobody listens) is replaced.
    drop(std::os::unix::net::UnixListener::bind(&sock).expect("stale socket"));
    let f = open(sock.clone()).await.expect("stale sockets are replaced");
    // Someone replaces the socket with a file while the forward runs: closing the forward leaves that file.
    std::fs::remove_file(&sock).expect("rm");
    std::fs::write(&sock, "theirs").expect("their file");
    drop(f);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(std::fs::read_to_string(&sock).expect("still theirs"), "theirs");
    assert_eq!(std::fs::read_to_string(&notes).expect("intact"), "keep me");
    assert_eq!(mode(&shared), 0o755);
}

#[cfg(unix)]
#[tokio::test]
async fn unix_forwards_are_private_and_named_after_the_lease() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, _conn, m) = start(vec![FakeDevice::iphone12(UDID_A)]).await;
    let h = m.get(&DeviceSelector::Only).expect("device");
    let f = russet_device::debug::open_forward_with(&h, russet_device::ForwardTarget::TunnelPort(1234), None)
        .await
        .expect("forward");
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

/// Canary (PLAN §0 DoD: no secrets in logs): device errors never carry a UDID, only its redacted form.
#[tokio::test]
async fn errors_never_carry_a_udid() {
    const CANARY: &str = "00008101-CA4A2700CA4A2700";
    let (_tmp, conn, m) = start(vec![FakeDevice::iphone12(UDID_A)]).await;
    let err = m.get(&DeviceSelector::Udid(Udid(CANARY.into()))).expect_err("not attached");
    let rendered =
        format!("{} {:?} {}", err.message(), err.to_data(), serde_json::to_string(&err.to_data()).expect("json"));
    assert!(!rendered.contains(CANARY), "{rendered}");
    assert!(rendered.contains("00008101-…"), "{rendered}");
    // The same for a device that goes away while a reservation is checked.
    conn.attach(FakeDevice::iphone12(CANARY));
    let mut rx = m.events();
    let h = loop {
        if let Ok(h) = m.get(&DeviceSelector::Udid(Udid(CANARY.into()))) {
            break h;
        }
        let _ = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    };
    m.reserve(&h, "c-1", "canary").expect("reserve");
    let err = m.check_access(h.udid(), "c-2", None).expect_err("reserved");
    assert!(!format!("{} {:?}", err.message(), err.to_data()).contains(CANARY));
}
