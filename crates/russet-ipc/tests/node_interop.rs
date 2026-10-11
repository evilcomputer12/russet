// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Interop with stock `vscode-jsonrpc` 9.0.3 (ARCHITECTURE §7.2, core §19 "Interop"; PLAN W2 tests): the Node
//! client of `tests/node/interop.mjs` runs `initialize`, `$/progress`, a cancellation (`-32800`), an unknown
//! method (`-32601`) and a `ui.confirm` it answers itself, against a russet-ipc server on a Unix socket.
//!
//! Needs `node` and `npm ci --prefix crates/russet-ipc/tests/node` (one 0.1 MB package). Without them the test
//! prints why and passes, unless `RUSSET_REQUIRE_NODE_INTEROP=1` (CI) makes that a failure.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use russet_ipc::{Endpoint, ListenOptions, Server};
use tokio_util::sync::CancellationToken;

fn node_dir() -> PathBuf {
    // The run-time CARGO_MANIFEST_DIR names the tree under test (shared target slots, AGENTS.md §6).
    let manifest =
        std::env::var_os("CARGO_MANIFEST_DIR").map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    manifest.join("tests/node")
}

fn skip_or_fail(why: &str) {
    if std::env::var_os("RUSSET_REQUIRE_NODE_INTEROP").is_some_and(|v| v == "1") {
        panic!("the vscode-jsonrpc interop test cannot run: {why}");
    }
    eprintln!("SKIPPED vscode-jsonrpc interop: {why}");
}

#[tokio::test]
async fn vscode_jsonrpc_9_speaks_russet() {
    let dir = node_dir();
    if std::process::Command::new("node").arg("--version").output().is_err() {
        return skip_or_fail("`node` is not on PATH");
    }
    if !dir.join("node_modules/vscode-jsonrpc/package.json").exists() {
        return skip_or_fail("run `npm ci --prefix crates/russet-ipc/tests/node` first");
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = tmp.path().join("run/russetd.sock");
    let server = Server::bind(&Endpoint::Unix(socket.clone()), ListenOptions::default()).await.expect("binds");
    let shutdown = CancellationToken::new();
    let serving = tokio::spawn(server.serve(support::router(), (), Arc::new(support::TestAuth), shutdown.clone()));

    let run = tokio::process::Command::new("node").arg("interop.mjs").arg(&socket).current_dir(&dir).output();
    let out = tokio::time::timeout(Duration::from_secs(30), run).await.expect("node finishes").expect("node runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "interop failed: {stdout}\n{stderr}");
    let summary: serde_json::Value = serde_json::from_str(stdout.lines().last().unwrap_or("")).expect("a JSON summary");
    assert_eq!(summary["ok"], true, "{summary}");
    assert_eq!(summary["results"]["cancelCode"], -32800);
    assert_eq!(summary["results"]["unknownCode"], -32601);
    assert_eq!(summary["results"]["confirm"], serde_json::json!({}));
    eprintln!("vscode-jsonrpc interop: {summary}");

    shutdown.cancel();
    serving.await.expect("join").expect("serve");
}
