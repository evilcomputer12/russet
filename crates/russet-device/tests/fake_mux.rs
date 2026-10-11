// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The live connector against a test-local fake usbmuxd (device.md §15.3; DEV-02 acceptance): a mux detach ends
//! the device's leases (forwards, reservations), and a restarted mux re-lists the device within 5 s. The fake
//! listens on a Unix socket in a temporary directory, set through `[device] mux = "usbmuxd:<path>"`; nothing here
//! touches `/var/run/usbmuxd` or a phone.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use russet_device::io::{LiveConnector, ServiceConnector};
use russet_device::mux::{encode_request, read_packet};
use russet_device::{DeviceConfig, DeviceEvent, DeviceManager, DeviceSelector, ForwardTarget};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

const UDID: &str = "00008101-000A1B2C3D4E5F60";

/// A mux event the fake sends to its `Listen` clients.
#[derive(Debug, Clone)]
enum Push {
    Attached(u32),
    Detached(u32),
}

fn attached(device_id: u32) -> plist::Dictionary {
    let mut props = plist::Dictionary::new();
    props.insert("ConnectionType".into(), "USB".into());
    props.insert("DeviceID".into(), device_id.into());
    props.insert("SerialNumber".into(), UDID.into());
    let mut d = plist::Dictionary::new();
    d.insert("MessageType".into(), "Attached".into());
    d.insert("DeviceID".into(), device_id.into());
    d.insert("Properties".into(), props.into());
    d
}

fn result(n: u64) -> plist::Dictionary {
    let mut d = plist::Dictionary::new();
    d.insert("MessageType".into(), "Result".into());
    d.insert("Number".into(), n.into());
    d
}

/// One run of the fake mux daemon; stopping it closes every connection, as a usbmuxd restart does.
struct FakeMux {
    stop: CancellationToken,
    push: broadcast::Sender<Push>,
    /// How many `Listen` sessions are open.
    listening: tokio::sync::watch::Receiver<usize>,
}

impl FakeMux {
    fn start(path: &Path, device_id: u32) -> Self {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).expect("bind the fake mux");
        let stop = CancellationToken::new();
        let (push, _) = broadcast::channel(16);
        let (listening_tx, listening) = tokio::sync::watch::channel(0usize);
        let listening_tx = Arc::new(listening_tx);
        let (s, p) = (stop.clone(), push.clone());
        tokio::spawn(async move {
            loop {
                let conn = tokio::select! {
                    _ = s.cancelled() => return,
                    c = listener.accept() => c,
                };
                let Ok((conn, _)) = conn else { continue };
                tokio::spawn(serve(conn, device_id, s.clone(), p.subscribe(), Arc::clone(&listening_tx)));
            }
        });
        Self { stop, push, listening }
    }

    /// Waits until the device plane listens for events.
    async fn wait_listening(&mut self) {
        tokio::time::timeout(Duration::from_secs(10), self.listening.wait_for(|n| *n > 0))
            .await
            .expect("the device plane listens in time")
            .expect("fake mux alive");
    }

    fn send(&self, p: Push) {
        let _ = self.push.send(p);
    }
}

impl Drop for FakeMux {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn serve(
    mut conn: tokio::net::UnixStream,
    device_id: u32,
    stop: CancellationToken,
    mut push: broadcast::Receiver<Push>,
    listening: Arc<tokio::sync::watch::Sender<usize>>,
) {
    loop {
        let req = tokio::select! {
            _ = stop.cancelled() => return,
            r = read_packet(&mut conn) => r,
        };
        let Ok((h, req)) = req else { return };
        let reply = |d: &plist::Dictionary, tag: u32| encode_request(d, tag).expect("encode");
        match req.get("MessageType").and_then(|v| v.as_string()) {
            Some("ListDevices") => {
                let mut list = plist::Dictionary::new();
                list.insert("DeviceList".into(), plist::Value::Array(vec![attached(device_id).into()]));
                let _ = conn.write_all(&reply(&list, h.tag)).await;
            }
            Some("Listen") => {
                let _ = conn.write_all(&reply(&result(0), h.tag)).await;
                let _ = conn.write_all(&reply(&attached(device_id), 0)).await;
                listening.send_modify(|n| *n += 1);
                loop {
                    let p = tokio::select! {
                        _ = stop.cancelled() => return,
                        p = push.recv() => p,
                    };
                    let msg = match p {
                        Ok(Push::Attached(id)) => attached(id),
                        Ok(Push::Detached(id)) => {
                            let mut d = plist::Dictionary::new();
                            d.insert("MessageType".into(), "Detached".into());
                            d.insert("DeviceID".into(), id.into());
                            d
                        }
                        Err(_) => return,
                    };
                    if conn.write_all(&reply(&msg, 0)).await.is_err() {
                        return;
                    }
                }
            }
            _ => {
                let _ = conn.write_all(&reply(&result(1), h.tag)).await;
            }
        }
    }
}

fn config(dir: &Path, mux: &Path) -> DeviceConfig {
    DeviceConfig {
        state_dir: dir.join("state"),
        runtime_dir: dir.join("runtime"),
        data_dir: dir.join("data"),
        config_dir: dir.join("config"),
        mux: format!("usbmuxd:{}", mux.display()),
        ..Default::default()
    }
}

async fn wait_for(rx: &mut broadcast::Receiver<DeviceEvent>, want: impl Fn(&DeviceEvent) -> bool) -> DeviceEvent {
    loop {
        let ev =
            tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("an event in time").expect("event");
        if want(&ev) {
            return ev;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mux_detach_ends_leases_and_a_restarted_mux_relists_within_5_s() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sock: PathBuf = tmp.path().join("usbmuxd.sock");
    let mut mux = FakeMux::start(&sock, 7);
    let cfg = config(tmp.path(), &sock);
    let conn = Arc::new(LiveConnector::new(&cfg).expect("connector"));
    let m = DeviceManager::start(cfg, conn as Arc<dyn ServiceConnector>).await.expect("start");
    let mut rx = m.events();
    mux.wait_listening().await;
    // The snapshot (ListDevices) already knows the device.
    assert_eq!(m.devices().len(), 1);
    let h = m.get(&DeviceSelector::Only).expect("the device");

    // Leases on the device: a forward and a reservation.
    let fwd = h
        .forward(ForwardTarget::TunnelPort(1234), russet_device::BindSpec::Tcp("127.0.0.1:0".parse().expect("a")))
        .await
        .expect("forward");
    let reservation = m.reserve(&h, "c-1", "fake-mux test").expect("reserve");

    // A mux detach removes the device and ends both.
    mux.send(Push::Detached(7));
    wait_for(&mut rx, |e| matches!(e, DeviceEvent::Removed(u) if u.as_str() == UDID)).await;
    tokio::time::timeout(Duration::from_secs(5), fwd.closed()).await.expect("the forward ended with the device");
    assert!(m.forwards().is_empty());
    assert!(!m.release(&reservation.lease), "the reservation ended with the device");

    // Re-attach (a new mux device id).
    mux.send(Push::Attached(8));
    wait_for(&mut rx, |e| matches!(e, DeviceEvent::Added(s) if s.udid.as_str() == UDID)).await;

    // Restart the mux: every connection drops, the socket goes, and it comes back 1.5 s later.
    let t = Instant::now();
    drop(mux);
    let _ = std::fs::remove_file(&sock);
    wait_for(&mut rx, |e| matches!(e, DeviceEvent::Removed(_))).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    mux = FakeMux::start(&sock, 9);
    wait_for(&mut rx, |e| matches!(e, DeviceEvent::Added(s) if s.udid.as_str() == UDID)).await;
    let relisted = t.elapsed();
    eprintln!("re-listed {relisted:?} after the mux went away (1.5 s restart)");
    assert!(relisted < Duration::from_secs(5), "re-listed after {relisted:?}");
    assert_eq!(m.devices().len(), 1);
    drop(mux);
}
