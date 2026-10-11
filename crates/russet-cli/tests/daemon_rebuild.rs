// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A rebuilt development binary meets the daemon of its previous build (ARCHITECTURE §3.3 item 3 [D 63, D 83];
//! PLAN W2 tests). A rebuild without a commit keeps the build id but replaces the executable (a new inode):
//! - the next CLI asks an idle daemon to step aside (`daemon.shutdown {reason: "upgrade"}`) and starts its own;
//! - a busy daemon (another client is connected) answers `ipc.buildMismatch` (1107, exit 12) with
//!   `restartable: false` and keeps running; once it is idle, the next CLI restarts it.
//!
//! The binary under test is copied into each test's temporary home, and the copy is "rebuilt" the way a linker
//! replaces its output: a new file renamed over the old one.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "daemon_support.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use support::{Home, alive, wait_gone};

/// Copies the binary under test to `<home>/bin/russet`.
fn install(home: &Home) -> PathBuf {
    let dir = home.root.join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("russet");
    rebuild(&home.bin, &dest);
    dest
}

/// Replaces `dest` with a fresh copy of `src`: the same content and build id, a new inode.
fn rebuild(src: &Path, dest: &Path) {
    let tmp = dest.with_extension("new");
    std::fs::copy(src, &tmp).unwrap();
    std::fs::rename(&tmp, dest).unwrap();
}

/// Runs `cmd`, retrying while another test thread's fork still holds the fresh copy open for writing (ETXTBSY).
fn output(cmd: &mut Command) -> Output {
    for _ in 0..200 {
        match cmd.output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(25))
            }
            r => return r.expect("russet runs"),
        }
    }
    panic!("the copied binary stayed busy");
}

/// `russet --json <args>` from `bin`: the exit code and the parsed stdout.
fn run_json(home: &Home, bin: &Path, args: &[&str]) -> (i32, Value) {
    let out = output(home.cmd_with(bin).arg("--json").args(args));
    let v = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "russet {args:?}: not JSON ({e}): {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().expect("exited"), v)
}

fn status_pid(v: &Value) -> u32 {
    u32::try_from(v["pid"].as_u64().unwrap_or_else(|| panic!("no pid in {v}"))).expect("pid")
}

#[test]
fn a_rebuild_without_a_commit_restarts_an_idle_daemon() {
    let home = Home::new("rebuild-idle");
    let bin = install(&home);
    let (code, first) = run_json(&home, &bin, &["daemon", "status"]);
    assert_eq!(code, 0, "{first}");
    let old = status_pid(&first);
    // The same executable: the same daemon.
    let (_, again) = run_json(&home, &bin, &["daemon", "status"]);
    assert_eq!(status_pid(&again), old);

    rebuild(&home.bin, &bin);
    let (code, second) = run_json(&home, &bin, &["daemon", "status"]);
    assert_eq!(code, 0, "{second}");
    let new = status_pid(&second);
    assert_ne!(new, old, "the stale daemon was replaced");
    assert_eq!(second["buildId"], first["buildId"], "no commit: the build id did not change");
    assert!(wait_gone(old, Duration::from_secs(5)), "the old daemon {old} exited");
    assert_eq!(home.daemon_pid(), Some(new));
}

#[test]
fn a_busy_daemon_of_another_executable_answers_1107_and_restarts_once_idle() {
    let home = Home::new("rebuild-busy");
    let bin = install(&home);
    let (_, first) = run_json(&home, &bin, &["daemon", "status"]);
    let old = status_pid(&first);

    // Another client stays connected: `daemon logs -f` of the same (old) executable.
    let mut follower = loop {
        match home.cmd_with(&bin).args(["daemon", "logs", "-f"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(25))
            }
            r => break r.expect("the follower starts"),
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, s) = run_json(&home, &bin, &["daemon", "status"]);
        let connected = s["connections"].as_array().is_some_and(|c| c.iter().any(|c| c["pid"] == follower.id()));
        if connected {
            break;
        }
        assert!(Instant::now() < deadline, "the follower never connected: {s}");
        std::thread::sleep(Duration::from_millis(50));
    }

    rebuild(&home.bin, &bin);
    let (code, v) = run_json(&home, &bin, &["daemon", "status"]);
    assert_eq!(code, 12, "{v}");
    assert_eq!(v["type"], "error", "{v}");
    let e = &v["error"];
    assert_eq!(e["code"], 1107, "{e}");
    assert_eq!(e["russetCode"], "ipc.buildMismatch");
    assert_eq!(e["details"]["restartable"], false, "{e}");
    assert_eq!(e["details"]["serverBuildId"], e["details"]["clientBuildId"], "{e}");
    assert!(alive(old), "a busy daemon keeps running");
    assert_eq!(home.daemon_pid(), Some(old));

    // The other client leaves: the daemon is idle, and the next CLI restarts it.
    follower.kill().expect("kill the follower");
    follower.wait().expect("reap the follower");
    let deadline = Instant::now() + Duration::from_secs(10);
    let new = loop {
        let (code, v) = run_json(&home, &bin, &["daemon", "status"]);
        if code == 0 {
            break status_pid(&v);
        }
        assert_eq!(v["error"]["code"], 1107, "{v}");
        assert!(Instant::now() < deadline, "the daemon never became idle: {v}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_ne!(new, old);
    assert!(wait_gone(old, Duration::from_secs(5)), "the old daemon {old} exited");
}
