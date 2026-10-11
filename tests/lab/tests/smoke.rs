// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The lab `smoke` suite (QA §3.9, PLAN §4.9): E2E-01, 02, 03, 04, 06 and 14 on the iPhone, adapted to the Wave-1
//! bundle-ID mapping (see the crate docs). Lane A only, so every test is `#[ignore]`d; `cargo xtask lab smoke` runs
//! them serially (`--ignored --test-threads=1`) after its pre-flight, with `RUSSET_BIN` set. The names sort in
//! the order they must run: E2E-14's cleanup comes last.

use std::time::Duration;

use russet_lab::{LabHarness, heartbeats, installed_bundle_ids};
use serde_json::Value;

fn harness() -> LabHarness {
    LabHarness::acquire().unwrap_or_else(|e| panic!("{e}"))
}

fn check(h: &LabHarness, args: &[&str]) -> Value {
    h.cli(args).and_then(|o| o.ok()).unwrap_or_else(|e| panic!("russet {}: {e}", args.join(" ")))
}

#[test]
#[ignore = "lane A: the iPhone (cargo xtask lab smoke)"]
fn e2e_01_doctor_ready() {
    let h = harness();
    let udid = h.udid().unwrap_or_else(|e| panic!("{e}"));
    let report = check(&h, &["--device", &udid, "doctor"]);
    for id in [
        "toolchain.swift",
        "toolchain.sdk",
        "toolchain.lldb",
        "usbmux.available",
        "device.pairing",
        "device.developerMode",
        "device.ddi",
    ] {
        let status = report["checks"].as_array().and_then(|c| c.iter().find(|x| x["id"] == id)).map(|x| &x["status"]);
        assert_eq!(status, Some(&Value::from("pass")), "{id}: {report}");
    }
    assert_eq!(report["summary"]["fail"], 0, "{}", report["summary"]);
}

#[test]
#[ignore = "lane A: the iPhone (cargo xtask lab smoke)"]
fn e2e_02_device_list() {
    let h = harness();
    let out = h.cli(&["device", "list"]).unwrap_or_else(|e| panic!("{e}"));
    assert!(out.elapsed < Duration::from_secs(5), "device list took {:?}", out.elapsed);
    let list = out.ok().unwrap_or_else(|e| panic!("{e}"));
    let d = &list["devices"][0];
    assert_eq!(d["productType"], "iPhone13,2", "{}", d["productType"]);
    assert_eq!(d["osVersion"], "27.0");
    assert_eq!(d["state"], "ready");
}

#[test]
#[ignore = "lane A: the iPhone (cargo xtask lab smoke)"]
fn e2e_03_build_install_launch() {
    let h = harness();
    let bundle = h.probe_bundle_id().unwrap_or_else(|e| panic!("{e}"));
    // Wave 1: the owner-local mapping onto the existing xtool profile (PLAN §4.10).
    assert!(bundle.starts_with("XTL-") && bundle.ends_with(".com.evilcomputer12.orchard.hello"), "bundle id");
    let started = std::time::Instant::now();
    let run = check(&h, &["run", "--detach"]);
    let session = run["sessionId"].as_str().expect("sessionId").to_owned();
    assert_eq!(run["bundleId"].as_str(), Some(bundle.as_str()));
    let mut seen = false;
    while started.elapsed() < Duration::from_secs(30) && !seen {
        let logs = h.cli(&["logs", "--session", &session, "--duration", "2s"]).unwrap_or_else(|e| panic!("{e}"));
        seen = logs.ndjson().iter().any(|e| e["message"].as_str().is_some_and(|m| m.starts_with("PROBE seq=")));
    }
    assert!(seen, "no PROBE line within 30 s");
    let apps = check(&h, &["app", "list"]);
    assert!(installed_bundle_ids(&apps).contains(&bundle), "the app list lacks the probe");
    check(&h, &["stop", "--all"]);
}

#[test]
#[ignore = "lane A: the iPhone (cargo xtask lab smoke)"]
fn e2e_04_breakpoint_cli() {
    let h = harness();
    let line = h.probe_break_line().unwrap_or_else(|e| panic!("{e}"));
    let at = format!("ContentView.swift:{line}");
    let out = h
        .cli(&["debug", "--batch", "--detach", "--break", &at, "--command", "frame variable probeValue"])
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(out.elapsed <= Duration::from_secs(60), "the breakpoint took {:?}", out.elapsed);
    let dbg = out.ok().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(dbg["stop"]["reason"], "breakpoint", "{dbg}");
    assert_eq!(dbg["stop"]["frame"]["line"], line);
    assert!(dbg["stop"]["frame"]["function"].as_str().is_some_and(|f| f.contains("ContentView.body.getter")));
    assert!(dbg["commands"][0]["output"].as_str().is_some_and(|o| o.contains("= 42")), "{}", dbg["commands"]);
    assert_eq!(dbg["detached"], true);
    std::thread::sleep(Duration::from_secs(5));
    let sessions = check(&h, &["sessions"]);
    let running: Vec<&Value> =
        sessions["sessions"].as_array().into_iter().flatten().filter(|s| s["state"] == "running").collect();
    assert_eq!(running.len(), 1, "{sessions}");
    assert_eq!(running[0]["debugger"], "detached");
    assert_eq!(running[0]["pid"], dbg["pid"]);
    check(&h, &["stop", "--all"]);
}

#[test]
#[ignore = "lane A: the iPhone (cargo xtask lab smoke)"]
fn e2e_06_logs() {
    let h = harness();
    let run = check(&h, &["run", "--detach"]);
    let bundle = run["bundleId"].as_str().expect("bundleId").to_owned();
    let logs = h.cli(&["logs", "--app", &bundle, "--duration", "15s"]).unwrap_or_else(|e| panic!("{e}"));
    let beats = heartbeats(&logs.ndjson());
    check(&h, &["stop", "--all"]);
    assert!(beats.longest_run >= 100, "{beats:?}");
    assert!(beats.p95_latency_ms.is_some_and(|p| p < 1000), "{beats:?}");
}

#[test]
#[ignore = "lane A: the iPhone (cargo xtask lab smoke)"]
fn e2e_14_cleanup() {
    let h = harness();
    let removed = h.cleanup().unwrap_or_else(|e| panic!("{e}"));
    eprintln!("removed {} dev.russet.test.* app(s)", removed.len());
    let apps = check(&h, &["app", "list"]);
    assert!(!installed_bundle_ids(&apps).iter().any(|b| b.starts_with("dev.russet.test.")));
    let sessions = check(&h, &["sessions"]);
    assert!(sessions["sessions"].as_array().into_iter().flatten().all(|s| s["state"] != "running"), "{sessions}");
}
