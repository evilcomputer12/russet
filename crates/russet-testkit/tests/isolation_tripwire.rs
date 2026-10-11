// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The lane-D isolation test (PLAN W8; ARCHITECTURE §3.3 item 7 [D 63, D 83]): a daemon started with
//! `RUSSET_DEVICE_BACKEND=scripted:…` and a tripwire `USBMUXD_SOCKET_ADDRESS` never connects to the tripwire,
//! even when a client lists and watches devices.
//!
//! It exercises W4's `DeviceManager` (through W2's daemon and `DeviceSlot`, which must also build the scripted
//! connector for `scripted:`; see the testkit README), and it cannot see a direct open of `/run/usbmuxd` (W2's
//! `live_connectors_constructed() == 0` assertion covers that). So it lands `#[ignore]`d; W9 enables it after W4
//! merges. `russet_bin()` builds this worktree's `russet` first:
//!
//! ```sh
//! cargo test -p russet-testkit --test isolation_tripwire -- --ignored
//! ```

#![cfg(unix)]

use std::time::Duration;

use russet_testkit::daemon::russet_bin;
use russet_testkit::{SpawnedDaemon, TempHome, Transcript};
use serde_json::json;

#[test]
#[ignore = "blocked: a spawned daemon cannot run the scripted backend until the lead moves ScriptedConnector where russetd may use it (testkit README); W9 ran it on 2026-10-11: device.list answers core.notImplemented"]
fn a_scripted_lane_d_daemon_never_connects_to_the_usbmuxd_tripwire() {
    let home = TempHome::builder().tripwire().build().expect("home");
    let daemon = SpawnedDaemon::spawn(&home, &russet_bin().expect("russet binary"), &[]).expect("daemon");
    let transcript = Transcript::default()
        .send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocol": {"major": 1, "minor": 0},
            "client": {"kind": "test", "name": "isolation-tripwire", "version": "0.1.0", "pid": std::process::id()},
            "capabilities": {"interactive": false}
        }}))
        .expect(json!({"id": 1, "result": {"server": {"deviceBackend": "$prefix:scripted:"}}}))
        .send(json!({"jsonrpc": "2.0", "id": 2, "method": "device.list", "params": {}}))
        .expect(json!({"id": 2, "result": "$any"}))
        .send(json!({"jsonrpc": "2.0", "id": 3, "method": "device.watch", "params": {}}))
        .expect(json!({"id": 3, "result": "$any"}))
        .ignoring(&["device.changed", "daemon.logBatch"]);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    rt.block_on(async {
        transcript.replay_unix(daemon.socket_path()).await.unwrap_or_else(|e| panic!("{e}\n{}", daemon.log()));
        // Give a background mux watcher, if anything started one, time to connect.
        tokio::time::sleep(Duration::from_millis(500)).await;
    });
    let tripwire = home.tripwire().expect("tripwire");
    tripwire.assert_untouched().unwrap_or_else(|e| panic!("{e}\n{}", daemon.log()));
    daemon.stop().expect("stop");
}
