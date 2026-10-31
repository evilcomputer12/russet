// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The audit log (ARCHITECTURE §6.5): every MCP call, CLI and IDE calls from class `account` upwards, params
//! redacted before they are written, `state/audit.jsonl` private.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use russet_core::rpc::ClientKind;
use russet_protocol::audit::{AuditKind, AuditTail, AuditTailParams};
use russet_protocol::daemon::{DaemonSetLogFilter, DaemonSetLogFilterParams, DaemonStatus, DaemonStatusParams};
use support::{Home, client, start};

const CANARY: &str = "RuSsEtAuDiTcAnArY91";

#[tokio::test]
async fn audited_calls_are_recorded_redacted() {
    let home = Home::new("audit");
    let (daemon, cli) = start(home.options()).await;
    let (agent, _) = daemon.connect(&client(ClientKind::Mcp, "codex", false)).await.expect("agent");
    // A read from MCP: audited. A read from the CLI: not.
    agent.call::<DaemonStatus>(DaemonStatusParams {}).await.expect("status");
    cli.call::<DaemonStatus>(DaemonStatusParams {}).await.expect("status");
    // A system call from the CLI: audited, with a canary in a sensitive field.
    cli.call_raw(
        "daemon.setLogFilter",
        serde_json::json!({"filter": "info", "token": CANARY, "note": format!("Bearer {CANARY}")}),
        russet_ipc::CallOptions::default(),
    )
    .await
    .expect("set");
    let _ = cli.call::<DaemonSetLogFilter>(DaemonSetLogFilterParams { filter: "info".into() }).await;
    let tail = cli
        .call::<AuditTail>(AuditTailParams { limit: Some(50), kinds: vec![AuditKind::Rpc], client: None })
        .await
        .expect("tail");
    let methods: Vec<(&str, &str)> = tail.entries.iter().map(|e| (e.client.name.as_str(), e.method.as_str())).collect();
    assert!(methods.contains(&("codex", "daemon.status")), "{methods:?}");
    assert!(!methods.contains(&("test-cli", "daemon.status")), "{methods:?}");
    assert!(methods.contains(&("test-cli", "daemon.setLogFilter")), "{methods:?}");
    let entry = tail.entries.iter().find(|e| e.method == "daemon.setLogFilter").expect("entry");
    assert_eq!(entry.outcome, "ok");
    assert_eq!(entry.params_redacted["token"], "***");
    assert_eq!(entry.params_digest.len(), 64);
    let text = std::fs::read_to_string(home.dirs.audit_log()).expect("audit.jsonl");
    assert!(!text.contains(CANARY), "the canary reached the audit log");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(home.dirs.audit_log()).unwrap().permissions().mode() & 0o777, 0o600);
    }
    daemon.stop().await.expect("stop");
}
