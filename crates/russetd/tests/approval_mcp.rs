// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Out-of-band approvals end to end (ARCHITECTURE §11.3; PLAN W2 tests): an `mcp` connection calling a tier-3
//! method gets `perm.approvalRequired` with an `approvalId`, cannot call `approval.grant`, and the binding
//! holds: a grant from a human covers exactly one call with the same arguments (mismatched and reused calls get
//! `perm.approvalInvalid`). Expiry is covered by the unit tests of the approval service. Under the default
//! policy's rate limit (`ship.upload` 1 per 10 min), the approved retry still runs: asking for an approval uses
//! no token.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_core::error::codes;
use russet_core::ids::ApprovalId;
use russet_core::rpc::ClientKind;
use russet_ipc::CallOptions;
use russet_protocol::approval::{
    ApprovalGet, ApprovalGetParams, ApprovalGrant, ApprovalGrantParams, ApprovalList, ApprovalListParams,
    ApprovalRequested, ApprovalResolved, ApprovalState,
};
use russet_protocol::audit::{AuditKind, AuditTail, AuditTailParams};
use russetd::McpPolicy;
use serde_json::json;
use support::{Home, client, start};

#[tokio::test]
async fn tier_three_calls_need_a_human_approval_bound_to_their_arguments() {
    let home = Home::new("approval-mcp");
    let mut o = home.options();
    let mut policy = McpPolicy::default();
    policy.clients.insert("claude-code".into(), 3);
    policy.rate_limits.clear();
    o.mcp_policy = policy;
    let (daemon, human) = start(o).await;
    let requested = Arc::new(Mutex::new(Vec::new()));
    let r = Arc::clone(&requested);
    human.on_notification::<ApprovalRequested, _>(move |p| r.lock().unwrap().push(p.approval));
    let resolved = Arc::new(Mutex::new(Vec::new()));
    let r = Arc::clone(&resolved);
    human.on_notification::<ApprovalResolved, _>(move |p| r.lock().unwrap().push(p.state));

    let (agent, init) = daemon.connect(&client(ClientKind::Mcp, "claude-code", false)).await.expect("agent");
    assert_eq!(init.grant.tier_max, Some(3));
    let params = json!({"project": "/p", "ipa": "/p/App.ipa"});
    let e = agent
        .call_raw("ship.upload", params.clone(), CallOptions::default())
        .await
        .expect_err("needs approval")
        .to_error();
    assert_eq!(e.code(), codes::perm::APPROVAL_REQUIRED);
    let details = e.details().cloned().expect("details");
    let id = ApprovalId::parse(details["approvalId"].as_str().expect("approvalId")).expect("apr_ id");
    assert_eq!(details["tier"], 3);
    assert!(details["approveWith"].as_str().is_some_and(|s| s.contains(id.as_str())));
    assert!(details["argsDigest"].as_str().is_some_and(|d| d.len() == 64));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(requested.lock().unwrap().iter().map(|a| a.approval_id.clone()).collect::<Vec<_>>(), vec![id.clone()]);

    // The agent cannot approve itself.
    let e = agent
        .call::<ApprovalGrant>(ApprovalGrantParams { approval_id: id.clone(), scope: None })
        .await
        .expect_err("denied");
    assert_eq!(e.code(), codes::perm::DENIED);
    // A CLI without a TTY cannot either.
    let (headless, _) = daemon.connect(&client(ClientKind::Cli, "script", false)).await.expect("cli");
    let e = headless
        .call::<ApprovalGrant>(ApprovalGrantParams { approval_id: id.clone(), scope: None })
        .await
        .expect_err("denied");
    assert_eq!(e.code(), codes::perm::DENIED);

    // A human grants it (a terminal CLI); approval.get sees the change.
    let waiter = {
        let human = human.clone();
        let id = id.clone();
        tokio::spawn(async move {
            human.call::<ApprovalGet>(ApprovalGetParams { approval_id: id, wait_ms: Some(5000) }).await
        })
    };
    let granted = human
        .call::<ApprovalGrant>(ApprovalGrantParams { approval_id: id.clone(), scope: None })
        .await
        .expect("granted");
    assert_eq!(granted.state, ApprovalState::Granted);
    assert_eq!(granted.granted_by.as_ref().map(|g| g.name.as_str()), Some("test-cli"));
    assert_eq!(waiter.await.expect("join").expect("get").state, ApprovalState::Granted);

    // Mismatched arguments.
    let mut other = json!({"project": "/p", "ipa": "/p/Other.ipa"});
    other["approval"] = json!(id.as_str());
    let e = agent.call_raw("ship.upload", other, CallOptions::default()).await.expect_err("mismatch").to_error();
    assert_eq!(e.code(), codes::perm::APPROVAL_INVALID);
    // The approved call passes the permission engine (and reaches W23's handler, not implemented yet).
    let mut approved = params.clone();
    approved["approval"] = json!(id.as_str());
    let e = agent
        .call_raw("ship.upload", approved.clone(), CallOptions::default())
        .await
        .expect_err("handler stub")
        .to_error();
    assert_eq!(e.code(), codes::core::NOT_IMPLEMENTED);
    // Reused.
    let e = agent.call_raw("ship.upload", approved, CallOptions::default()).await.expect_err("reused").to_error();
    assert_eq!(e.code(), codes::perm::APPROVAL_INVALID);
    let used = human.call::<ApprovalList>(ApprovalListParams { state: Some(ApprovalState::Used) }).await.expect("list");
    assert!(used.approvals.iter().any(|a| a.approval_id == id));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let states = resolved.lock().unwrap().clone();
    assert!(states.contains(&ApprovalState::Granted) && states.contains(&ApprovalState::Used), "{states:?}");

    // Every MCP call is audited, refusals included.
    let audit = human
        .call::<AuditTail>(AuditTailParams { client: Some("claude-code".into()), limit: Some(100), kinds: vec![] })
        .await
        .expect("audit");
    let outcomes: Vec<(AuditKind, &str)> = audit.entries.iter().map(|e| (e.kind, e.outcome.as_str())).collect();
    for expected in [
        (AuditKind::Rpc, "perm.approvalRequired"),
        (AuditKind::Rpc, "perm.denied"),
        (AuditKind::Rpc, "perm.approvalInvalid"),
        (AuditKind::Rpc, "core.notImplemented"),
        (AuditKind::Approval, "granted"),
        (AuditKind::Approval, "used"),
    ] {
        assert!(outcomes.contains(&expected), "{expected:?} missing in {outcomes:?}");
    }
    daemon.stop().await.expect("stop");
}

#[tokio::test]
async fn the_approved_retry_runs_under_the_default_rate_limit() {
    let home = Home::new("approval-rate");
    let mut o = home.options();
    let mut policy = McpPolicy::default();
    assert!(policy.rate_limits.contains_key("ship.upload"), "the default policy limits ship.upload");
    policy.clients.insert("claude-code".into(), 3);
    o.mcp_policy = policy;
    let (daemon, human) = start(o).await;
    let (agent, _) = daemon.connect(&client(ClientKind::Mcp, "claude-code", false)).await.expect("agent");
    let params = json!({"project": "/p", "ipa": "/p/App.ipa"});
    let e = agent
        .call_raw("ship.upload", params.clone(), CallOptions::default())
        .await
        .expect_err("needs approval")
        .to_error();
    assert_eq!(e.code(), codes::perm::APPROVAL_REQUIRED);
    let id = ApprovalId::parse(e.details().and_then(|d| d["approvalId"].as_str()).expect("approvalId")).expect("id");
    // Asking again, or presenting a bad approval, still uses no token.
    let e = agent.call_raw("ship.upload", params.clone(), CallOptions::default()).await.expect_err("again").to_error();
    assert_eq!(e.code(), codes::perm::APPROVAL_REQUIRED);
    let mut wrong = params.clone();
    wrong["approval"] = json!("apr_01J00000000000000000000000");
    let e = agent.call_raw("ship.upload", wrong, CallOptions::default()).await.expect_err("unknown").to_error();
    assert_eq!(e.code(), codes::perm::APPROVAL_INVALID);
    human.call::<ApprovalGrant>(ApprovalGrantParams { approval_id: id.clone(), scope: None }).await.expect("granted");
    let mut approved = params.clone();
    approved["approval"] = json!(id.as_str());
    let e = agent
        .call_raw("ship.upload", approved, CallOptions::default())
        .await
        .expect_err("W23's handler is a stub")
        .to_error();
    assert_eq!(e.code(), codes::core::NOT_IMPLEMENTED, "the approved retry passed the permission engine: {e:?}");
    // That call used the token: the next one is limited before a human is asked.
    let pending = |list: Vec<russet_protocol::approval::Approval>| list.len();
    let before = pending(human.call::<ApprovalList>(ApprovalListParams { state: None }).await.expect("list").approvals);
    let e = agent
        .call_raw("ship.upload", json!({"project": "/p", "ipa": "/p/Next.ipa"}), CallOptions::default())
        .await
        .expect_err("limited")
        .to_error();
    assert_eq!(e.code(), codes::perm::RATE_LIMITED);
    assert!(e.is_retryable());
    let after = pending(human.call::<ApprovalList>(ApprovalListParams { state: None }).await.expect("list").approvals);
    assert_eq!(before, after, "a limited call creates no approval");
    daemon.stop().await.expect("stop");
}

#[tokio::test]
async fn tiers_above_tier_max_and_human_only_classes_are_refused() {
    let home = Home::new("approval-tiers");
    let (daemon, _cli) = start(home.options()).await;
    let (agent, init) = daemon.connect(&client(ClientKind::Mcp, "local-llama", false)).await.expect("agent");
    assert_eq!(init.grant.tier_max, Some(1), "the default tier-max [D 60]");
    let e = agent.call_raw("ship.upload", json!({}), CallOptions::default()).await.expect_err("tier 3").to_error();
    assert_eq!(e.code(), codes::perm::TIER_EXCEEDED);
    let e = agent
        .call_raw("portal.devices.register", json!({}), CallOptions::default())
        .await
        .expect_err("tier 2")
        .to_error();
    assert_eq!(e.code(), codes::perm::TIER_EXCEEDED);
    for (method, params) in [("daemon.shutdown", json!({})), ("secrets.delete", json!({"id": "x"}))] {
        let e = agent.call_raw(method, params, CallOptions::default()).await.expect_err("never").to_error();
        assert_eq!(e.code(), codes::perm::DENIED, "{method}");
    }
    agent.call_raw("daemon.status", json!({}), CallOptions::default()).await.expect("tier 0");
    daemon.stop().await.expect("stop");
}
