// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Development isolation across processes (ARCHITECTURE §3.3 item 7 [D 63, D 83]; PLAN W2 tests; AGENTS.md §4),
//! with real `russet daemon run` processes:
//! - a development daemon asked for `live` without `RUSSET_LANE=A` (here: autostarted by the CLI) answers every
//!   device method with `device.liveBackendRefused` (3019, `reason: development-build`) and never takes the lock;
//! - with `--allow-live-device`, the first daemon takes the host-wide live-device lock and a second one answers
//!   3019 with `details.holder {pid, home}` of the first; once the holder stops, the next live daemon gets it.
//!
//! The lock is a temporary file (`RUSSET_LIVE_DEVICE_LOCK`, honoured by debug builds only) and
//! `USBMUXD_SOCKET_ADDRESS` points at a fake usbmuxd that counts connections: none of these daemons may open
//! one. Nothing here sets `RUSSET_LANE=A` or touches the real lock.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "daemon_support.rs"]
mod support;

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use russet_core::rpc::ClientKind;
use russet_ipc::{CallOptions, ClientInfo, Endpoint};
use serde_json::{Value, json};
use support::{FakeUsbmuxd, Home, wait_gone};

/// The live environment of `home`: the temporary lock and the fake usbmuxd.
fn live(home: &Home, lock: &Path, mux: &FakeUsbmuxd) -> Command {
    let mut c = home.cmd();
    c.env("RUSSET_DEVICE_BACKEND", "live")
        .env("RUSSET_LIVE_DEVICE_LOCK", lock)
        .env("USBMUXD_SOCKET_ADDRESS", &mux.path);
    c
}

/// Starts `russet daemon run --allow-live-device` in `home` and waits until it answers.
#[allow(clippy::zombie_processes, reason = "the child goes to the caller, and `stop` waits for it")]
fn run_live_daemon(home: &Home, lock: &Path, mux: &FakeUsbmuxd) -> (Child, u32) {
    let mut child = live(home, lock, mux)
        .args(["daemon", "run", "--allow-live-device", "--idle-timeout", "0"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the daemon starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = home.daemon_pid()
            && call(home, "daemon.ping", json!({})).is_ok()
        {
            assert_eq!(pid, child.id(), "the daemon of {} is the one this test started", home.root.display());
            return (child, pid);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let status = child.wait();
            panic!("the live daemon in {} did not start ({status:?})", home.root.display());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// One call to the daemon of `home` as a CLI client, through the discovery file's endpoint.
fn call(home: &Home, method: &str, params: Value) -> Result<Value, russet_core::error::Error> {
    let text = std::fs::read_to_string(home.runtime().join("russetd.json"))
        .map_err(|e| russet_core::error::Error::io("reading russetd.json", e))?;
    let discovery: Value =
        serde_json::from_str(&text).map_err(|e| russet_core::error::Error::internal(e.to_string()))?;
    let endpoint = Endpoint::parse(discovery["endpoint"].as_str().unwrap_or_default())?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    rt.block_on(async {
        let (peer, _) = russet_ipc::connect(&endpoint, ClientInfo::new(ClientKind::Cli, "live-lock-test")).await?;
        let result = peer.call_raw(method, params, CallOptions::default()).await.map_err(|e| e.to_error());
        peer.close();
        result
    })
}

fn stop(home: &Home, mut child: Child, pid: u32) {
    let out = home.run(&["--json", "daemon", "stop", "--force"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    child.wait().expect("reap the daemon");
    assert!(wait_gone(pid, Duration::from_secs(5)));
}

#[test]
fn a_development_daemon_without_lane_a_refuses_live_with_3019() {
    let scratch = russet_core::fsx::TempDir::new("live-refused").expect("scratch");
    let lock = scratch.path().join("live-device.lock");
    let mux = FakeUsbmuxd::start(scratch.path().join("usbmuxd.sock"));
    let home = Home::new("live-refused-home");
    // The CLI autostarts a daemon with its environment: `live`, no RUSSET_LANE, no --allow-live-device.
    let out = live(&home, &lock, &mux).args(["--json", "daemon", "status"]).output().expect("russet runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let status: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(status["deviceBackend"], "live");
    for method in ["device.list", "app.list", "logs.tail"] {
        let e = call(&home, method, json!({})).expect_err("refused");
        assert_eq!(e.code().number(), 3019, "{method}: {e:?}");
        assert_eq!(e.details().expect("details")["reason"], "development-build", "{method}");
    }
    assert!(!lock.exists(), "a refused daemon never takes the live-device lock");
    assert_eq!(mux.connections(), 0, "a refused daemon never connects to usbmuxd");
}

#[test]
fn a_second_live_daemon_gets_3019_with_the_holder() {
    let scratch = russet_core::fsx::TempDir::new("live-lock").expect("scratch");
    let lock = scratch.path().join("live-device.lock");
    let mux = FakeUsbmuxd::start(scratch.path().join("usbmuxd.sock"));
    let first = Home::new("live-first");
    let second = Home::new("live-second");

    let (first_child, first_pid) = run_live_daemon(&first, &lock, &mux);
    // The holder passes the guard: `device.list` reaches the device plane (W4's handler; a stub until then).
    let reached = call(&first, "device.list", json!({}));
    assert!(not_refused(&reached), "{reached:?}");

    let (second_child, second_pid) = run_live_daemon(&second, &lock, &mux);
    let e = call(&second, "device.list", json!({})).expect_err("the lock is held");
    assert_eq!(e.code().number(), 3019, "{e:?}");
    let holder = e.details().and_then(|d| d.get("holder")).cloned().expect("details.holder");
    assert_eq!(holder["pid"], first_pid);
    assert_eq!(holder["home"], first.root.display().to_string());
    let status = call(&second, "daemon.status", json!({})).expect("the second daemon runs");
    assert_eq!(status["deviceBackend"], "live");

    // The holder stops: the next live daemon takes the lock.
    stop(&second, second_child, second_pid);
    stop(&first, first_child, first_pid);
    let third = Home::new("live-third");
    let (third_child, third_pid) = run_live_daemon(&third, &lock, &mux);
    let reached = call(&third, "device.list", json!({}));
    assert!(not_refused(&reached), "{reached:?}");
    stop(&third, third_child, third_pid);
    assert_eq!(mux.connections(), 0, "no daemon connected to usbmuxd");
}

/// `Ok`, or an error other than 3019 (the device plane's own answer).
fn not_refused(r: &Result<Value, russet_core::error::Error>) -> bool {
    r.as_ref().map_or_else(|e| e.code().number() != 3019, |_| true)
}
