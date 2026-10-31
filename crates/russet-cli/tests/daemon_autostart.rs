// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Autostart, single instance, idle exit and `--json` output of the real binary (ARCHITECTURE §3.3; PLAN W2
//! tests and acceptance): 8 concurrent CLIs result in exactly one daemon, two `RUSSET_HOME`s give two daemons, an
//! idle daemon exits, `russet daemon status --json` validates against `schemas/rpc/v1`, and `--no-daemon` works
//! in-process.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use std::time::Duration;

use support::{Home, alive, daemons_of, protocol_schema, validate, wait_gone};

#[test]
fn eight_concurrent_clis_start_exactly_one_daemon() {
    let home = Home::new("autostart-8");
    let children: Vec<std::process::Child> = (0..8)
        .map(|_| {
            home.cmd()
                .args(["--json", "daemon", "status"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn")
        })
        .collect();
    let mut pids = std::collections::BTreeSet::new();
    for child in children {
        let out = child.wait_with_output().expect("wait");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
        pids.insert(v["pid"].as_u64().expect("pid"));
    }
    assert_eq!(pids.len(), 1, "every CLI talked to the same daemon: {pids:?}");
    let pid = u32::try_from(*pids.iter().next().expect("one")).expect("pid");
    assert_eq!(home.daemon_pid(), Some(pid));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(daemons_of(&home.root), vec![pid], "exactly one daemon process for the home");
}

#[test]
fn two_homes_give_two_daemons() {
    let a = Home::new("autostart-a");
    let b = Home::new("autostart-b");
    let pa = a.json(&["daemon", "status"])["pid"].as_u64();
    let pb = b.json(&["daemon", "status"])["pid"].as_u64();
    assert!(pa.is_some() && pb.is_some());
    assert_ne!(pa, pb);
    assert!(a.json(&["daemon", "status"])["endpoint"].as_str().is_some_and(|e| e.contains(a.root.to_str().unwrap())));
}

#[test]
fn an_idle_daemon_exits() {
    let home = Home::new("autostart-idle");
    let started = home.json(&["daemon", "start", "--idle-timeout", "1s"]);
    let pid = u32::try_from(started["pid"].as_u64().expect("pid")).expect("pid");
    assert!(alive(pid));
    assert!(wait_gone(pid, Duration::from_secs(8)), "the idle daemon {pid} did not exit");
    assert!(!home.runtime().join("russetd.json").exists(), "russetd.json is removed");
    assert!(!home.runtime().join("russetd.sock").exists(), "the socket is removed");
}

#[test]
fn daemon_status_json_validates_against_the_protocol_schema() {
    let home = Home::new("autostart-schema");
    let v = home.json(&["daemon", "status"]);
    let schema = protocol_schema();
    let result = &schema["methods"]["daemon.status"]["result"];
    validate(result, &v, &schema["$defs"], "daemon.status").expect("the output validates");
    // The validator does reject a wrong document.
    let mut broken = v.clone();
    broken["pid"] = serde_json::json!("not a number");
    assert!(validate(result, &broken, &schema["$defs"], "daemon.status").is_err());
    // The discovery file is private and complete.
    let text = std::fs::read_to_string(home.runtime().join("russetd.json")).expect("russetd.json");
    let d: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    for key in ["pid", "version", "buildId", "protocol", "endpoint", "startedAt", "deviceBackend"] {
        assert!(d.get(key).is_some(), "{key} missing in russetd.json");
    }
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&home.runtime().join("russetd.json")), 0o600);
    assert_eq!(mode(&home.runtime().join("russetd.sock")), 0o600);
    assert_eq!(mode(&home.runtime()), 0o700);
}

#[test]
fn no_daemon_runs_in_process() {
    let home = Home::new("autostart-inproc");
    let child = home
        .cmd()
        .args(["--json", "--no-daemon", "daemon", "status"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let cli_pid = child.id();
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(v["endpoint"], "inproc");
    assert_eq!(v["pid"].as_u64(), Some(u64::from(cli_pid)), "the in-process daemon is the CLI process itself");
    assert!(!home.runtime().join("russetd.sock").exists(), "no socket for an in-process daemon");
    assert!(daemons_of(&home.root).is_empty());
    let human = home.run(&["--no-daemon", "daemon", "status"]);
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stdout).contains("endpoint: inproc"));
}

/// `RUSSET_DAEMON=inproc` is `--no-daemon` (ARCHITECTURE §14.9).
#[test]
fn russet_daemon_inproc_runs_in_process() {
    let home = Home::new("autostart-inproc-env");
    let child = home
        .cmd()
        .env("RUSSET_DAEMON", "inproc")
        .args(["--json", "daemon", "status"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let cli_pid = child.id();
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(v["endpoint"], "inproc");
    assert_eq!(v["pid"].as_u64(), Some(u64::from(cli_pid)));
    assert!(daemons_of(&home.root).is_empty(), "nothing was spawned");
    let path = home.cmd().env("RUSSET_DAEMON", "inproc").args(["daemon", "path"]).output().expect("runs");
    assert!(path.status.success());
    assert!(String::from_utf8_lossy(&path.stdout).contains("inproc"));
    // There is no daemon process to stop.
    let stop = home.cmd().env("RUSSET_DAEMON", "inproc").args(["--json", "daemon", "stop"]).output().expect("runs");
    assert!(stop.status.success());
    let v: serde_json::Value = serde_json::from_slice(&stop.stdout).expect("JSON");
    assert_eq!(v["stopped"], false);
}
