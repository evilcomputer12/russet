// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The Node interop harness (vscode-jsonrpc 9.0.3) against the fake daemon: every case W2's daemon test uses
//! (PLAN W2: initialize, `$/progress`, cancel → -32800, unknown method → -32601, a `ui.confirm` answered by the
//! client). Skipped with a note when node or the harness's node_modules is missing, unless
//! `RUSSET_REQUIRE_NODE_HARNESS=1` (CI).

#![cfg(unix)]

use russet_testkit::fake_daemon::FakeDaemon;
use russet_testkit::node::{self, InteropPlan, NodeError};
use serde_json::json;

fn harness_or_skip() -> bool {
    match node::available() {
        Ok(()) => true,
        Err(e) if !node::required() => {
            eprintln!("skipped: {e}");
            false
        }
        Err(e) => panic!("{e} (RUSSET_REQUIRE_NODE_HARNESS=1)"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn vscode_jsonrpc_speaks_russet_1() {
    if !harness_or_skip() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let daemon = FakeDaemon::start(dir.path()).await.expect("fake daemon");
    let plan = InteropPlan::full(daemon.socket_path(), "test.slow", json!({}), "test.confirm", json!({"title": "Go?"}));
    let report = tokio::task::spawn_blocking(move || node::run(&plan)).await.expect("join").expect("harness");
    assert!(report.ok, "{report:#?}");
    let init = report.step("initialize").and_then(|s| s.result.clone()).expect("initialize result");
    assert_eq!(init["protocol"], json!({"major": 1, "minor": 0}));
    let unknown = report.step("unknown-method").expect("step");
    assert_eq!(unknown.error.as_ref().map(|e| e["code"].clone()), Some(json!(-32601)));
    let cancel = report.step("progress-and-cancel").expect("step");
    assert!(cancel.progress.as_ref().is_some_and(|p| p[0]["kind"] == json!("begin")), "{cancel:?}");
    assert_eq!(cancel.error.as_ref().map(|e| e["code"].clone()), Some(json!(-32800)));
    assert_eq!(cancel.error.as_ref().map(|e| e["data"]["russetCode"].clone()), Some(json!("core.cancelled")));
    let confirm = report.step("ui-confirm").expect("step");
    assert_eq!(confirm.confirm.as_ref().map(|c| c["title"].clone()), Some(json!("Go?")));
    assert_eq!(confirm.result.as_ref().map(|r| r["confirmed"].clone()), Some(json!(true)));
    // The fake saw what vscode-jsonrpc sent: initialize first, then the cancel notification.
    let methods: Vec<String> =
        daemon.received().iter().filter_map(|m| m["method"].as_str().map(str::to_owned)).collect();
    assert_eq!(methods.first().map(String::as_str), Some("initialize"));
    assert!(methods.iter().any(|m| m == "$/cancelRequest"), "{methods:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_confirmation_and_a_failing_step_are_reported() {
    if !harness_or_skip() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let daemon = FakeDaemon::start(dir.path()).await.expect("fake daemon");
    let plan = InteropPlan::new(daemon.socket_path())
        .confirm_answer(false)
        .step(node::InteropStep::confirm("declined", "test.confirm", json!({})))
        .step(node::InteropStep::request("missing", "russet.noSuchMethod", json!({})));
    let report = tokio::task::spawn_blocking(move || node::run(&plan)).await.expect("join").expect("harness");
    assert!(!report.ok, "a failing step fails the report");
    let declined = report.step("declined").expect("step");
    assert!(declined.ok, "confirmed = false matches the declined answer: {declined:?}");
    assert!(!report.step("missing").expect("step").ok);
}

#[test]
fn an_unreachable_socket_is_a_report_not_a_hang() {
    if !harness_or_skip() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let mut plan = InteropPlan::new(&dir.path().join("nobody-listens.sock"));
    plan.timeout_ms = 2000;
    match node::run(&plan) {
        Ok(r) => assert!(!r.ok && r.fatal.is_some(), "{r:#?}"),
        Err(NodeError::BadOutput { .. }) => {}
        Err(e) => panic!("{e}"),
    }
}
