// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The end-to-end redaction canary (PLAN §0 definition of done "no secrets in logs (canary test)"; ARCHITECTURE
//! §6.4 redaction, §7.5 audit): a real daemon logging at `RUSSET_LOG=trace` serves a client whose name and
//! parameters carry a canary in every shape a secret takes (sensitive field names, `Bearer`, a JWT, a PEM block,
//! a `token=` assignment), through audited, refused, unknown and malformed calls. Afterwards no file under
//! `RUSSET_HOME` holds the canary, while the logs do hold the trace-level records it went through.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "daemon_support.rs"]
mod support;

use std::path::{Path, PathBuf};

use russet_core::rpc::ClientKind;
use russet_ipc::{CallOptions, ClientInfo, Endpoint};
use serde_json::{Value, json};
use support::Home;

const CANARY: &str = "RuSsEtE2eCaNaRy5c1";

/// Every regular file under `dir`.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            out.extend(files(&e.path()));
        } else if kind.is_file() {
            out.push(e.path());
        }
    }
    out
}

#[test]
fn no_file_of_a_trace_level_daemon_holds_the_canary() {
    let home = Home::new("canary-e2e");
    let out = home.cmd().env("RUSSET_LOG", "trace").args(["--json", "daemon", "start"]).output().expect("russet runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let started: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let endpoint = Endpoint::parse(started["endpoint"].as_str().expect("endpoint")).expect("endpoint");

    let shapes = json!({
        "token": CANARY,
        "password": CANARY,
        "note": format!("Authorization: Bearer {CANARY}"),
        "jwt": format!("eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJ{CANARY}.c2ln"),
        "pem": format!("-----BEGIN PRIVATE KEY-----\n{CANARY}\n-----END PRIVATE KEY-----"),
        "url": format!("https://example.invalid/cb?token={CANARY}&x=1"),
    });
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    rt.block_on(async {
        let client = ClientInfo::new(ClientKind::Cli, format!("Bearer {CANARY}"));
        let (peer, _) = russet_ipc::connect(&endpoint, client).await.expect("connects");
        let call = |method: &'static str, params: Value| {
            let peer = peer.clone();
            async move { peer.call_raw(method, params, CallOptions::default()).await }
        };
        // An audited system call that succeeds, with the canary next to its real parameter.
        let mut p = shapes.clone();
        p["filter"] = json!("trace");
        call("daemon.setLogFilter", p).await.expect("the filter call succeeds");
        // A refused filter whose text is the secret.
        call("daemon.setLogFilter", json!({"filter": format!("token={CANARY}")})).await.expect_err("not a filter");
        // A human-only method from an unprompted CLI, an unknown method and malformed parameters.
        let mut p = shapes.clone();
        p["approvalId"] = json!("apr_01J00000000000000000000000");
        call("approval.grant", p).await.expect_err("denied");
        call("no.such.method", shapes.clone()).await.expect_err("unknown");
        call("job.list", json!({"limit": format!("Bearer {CANARY}")})).await.expect_err("malformed");
        // A notification and a cancellation for a request that does not exist.
        peer.notify_raw("$/cancelRequest", json!({"id": CANARY})).expect("sent");
        peer.notify_raw("no.such.notification", shapes.clone()).expect("sent");
        call("daemon.ping", json!({})).await.expect("still serving");
        peer.close();
    });

    // Stopping flushes every writer.
    let stopped = home.json(&["daemon", "stop", "--force"]);
    assert_eq!(stopped["stopped"], true, "{stopped}");

    let all = files(&home.root);
    let logs: Vec<&PathBuf> = all.iter().filter(|p| p.extension().is_some_and(|e| e == "log")).collect();
    assert!(!logs.is_empty(), "the daemon wrote logs: {all:?}");
    let log_text: String = logs.iter().map(|p| std::fs::read_to_string(p).unwrap_or_default()).collect();
    assert!(log_text.contains("connection initialized"), "trace-level records were written:\n{log_text}");
    assert!(log_text.contains("log filter changed"), "{log_text}");
    let audit = std::fs::read_to_string(home.root.join("state").join("audit.jsonl")).unwrap_or_default();
    assert!(
        audit.contains("approval.grant") && audit.contains("daemon.setLogFilter"),
        "the calls were audited:\n{audit}"
    );
    for file in &all {
        let bytes = std::fs::read(file).unwrap_or_default();
        assert!(
            !bytes.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()),
            "the canary reached {}:\n{}",
            file.display(),
            String::from_utf8_lossy(&bytes)
        );
    }
}
