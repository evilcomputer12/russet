// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Exit codes and the `--json` error shape of the real binary (ARCHITECTURE §13.1, §13.4; PLAN W2 acceptance
//! "the exit-code table is covered by tests"). The unit tests of `russet_cli` map every family; these run the
//! binary end to end.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use support::Home;

fn code(out: &std::process::Output) -> i32 {
    out.status.code().expect("exited")
}

fn json_error(out: &std::process::Output) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("a JSON error on stdout");
    assert_eq!(v["type"], "error", "{v}");
    v["error"].clone()
}

#[test]
fn usage_errors_exit_2_and_success_exits_0() {
    let home = Home::new("exit-usage");
    assert_eq!(code(&home.run(&["daemon", "frobnicate"])), 2);
    assert_eq!(code(&home.run(&["--no-such-flag"])), 2);
    assert_eq!(code(&home.run(&["--json", "daemon", "path"])), 0);
    assert_eq!(code(&home.run(&["--version"])), 0);
}

#[test]
fn an_unreachable_daemon_exits_12_with_a_json_error() {
    let home = Home::new("exit-12");
    let out = home.cmd().env("RUSSET_NO_AUTOSTART", "1").args(["--json", "daemon", "status"]).output().expect("runs");
    assert_eq!(code(&out), 12);
    let e = json_error(&out);
    assert_eq!(e["code"], 1102);
    assert_eq!(e["russetCode"], "ipc.daemonUnavailable");
    assert!(e["hint"].as_str().is_some_and(|h| h.contains("russet daemon start")), "{e}");
    // Human mode: the error line of ARCHITECTURE §13.1 on stderr.
    let out = home.cmd().env("RUSSET_NO_AUTOSTART", "1").args(["daemon", "status"]).output().expect("runs");
    assert_eq!(code(&out), 12);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ipc.daemonUnavailable (1102):") && stderr.contains("Hint:"), "{stderr}");
}

#[test]
fn errors_map_to_their_exit_codes() {
    let home = Home::new("exit-map");
    // core.invalidArgument (a malformed id): 1.
    let out = home.run(&["--json", "--no-daemon", "approve", "apr_bad"]);
    assert_eq!(code(&out), 1);
    assert_eq!(json_error(&out)["russetCode"], "core.invalidArgument");
    // perm.denied (a grant from a CLI without a TTY): 11.
    let out = home.run(&["--json", "--no-daemon", "approve", "apr_01J00000000000000000000000"]);
    assert_eq!(code(&out), 11);
    assert_eq!(json_error(&out)["russetCode"], "perm.denied");
    // core.notImplemented (a later wave's stub command): 1.
    let out = home.run(&["--json", "crash"]);
    assert_eq!(code(&out), 1);
    assert_eq!(json_error(&out)["russetCode"], "core.notImplemented");
    // Successful daemon-backed commands: 0, with one JSON document.
    let v = home.json(&["--no-daemon", "approvals", "list"]);
    assert_eq!(v, serde_json::json!({"approvals": []}));
    // The denied grant above is in the audit log (ARCHITECTURE §7.5), with its parameters redacted.
    let v = home.json(&["--no-daemon", "audit", "tail"]);
    let entries = v["entries"].as_array().expect("entries");
    assert!(
        entries
            .iter()
            .any(|e| e["method"] == "approval.grant" && e["outcome"] == "perm.denied" && e["client"]["kind"] == "cli"),
        "{v}"
    );
    // Stopping a daemon that does not run is fine.
    let v = home.json(&["daemon", "stop"]);
    assert_eq!(v["stopped"], false);
}
