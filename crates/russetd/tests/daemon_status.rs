// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `daemon.*` (ARCHITECTURE §8.1, §9.1): ping, status, shutdown (refused with active work unless forced),
//! the log filter and `daemon.logs.subscribe` batches.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_core::error::codes;
use russet_core::rpc::ClientKind;
use russet_protocol::daemon::{
    DaemonLogBatch, DaemonLogRecord, DaemonLogsSubscribe, DaemonLogsSubscribeParams, DaemonPing, DaemonPingParams,
    DaemonSetLogFilter, DaemonSetLogFilterParams, DaemonShutdown, DaemonShutdownParams, DaemonStatus,
    DaemonStatusParams, ShutdownReason, TraceLevel,
};
use russet_protocol::subscription::{SubscriptionCancel, SubscriptionCancelParams};
use support::{Home, client, start};

#[tokio::test]
async fn ping_and_status_describe_the_daemon() {
    let home = Home::new("daemon-status");
    let (daemon, cli) = start(home.options()).await;
    cli.call::<DaemonPing>(DaemonPingParams {}).await.expect("ping");
    let s = cli.call::<DaemonStatus>(DaemonStatusParams {}).await.expect("status");
    assert_eq!(s.pid, std::process::id());
    assert_eq!(s.endpoint, "inproc");
    assert_eq!(s.device_backend, "none");
    assert_eq!(s.build_id, russet_core::build_info::BUILD_ID);
    assert_eq!(s.connections.len(), 1);
    assert_eq!((s.connections[0].kind, s.connections[0].name.as_str()), (ClientKind::Cli, "test-cli"));
    assert_eq!((s.jobs.queued, s.jobs.running, s.devices), (0, 0, 0));
    #[cfg(target_os = "linux")]
    assert!(s.rss_bytes > 0);
    // A second connection shows up, and goes away when it closes.
    let (ide, _) = daemon.connect(&client(ClientKind::Ide, "russet-ide", false)).await.expect("ide");
    assert_eq!(cli.call::<DaemonStatus>(DaemonStatusParams {}).await.expect("status").connections.len(), 2);
    ide.close();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(cli.call::<DaemonStatus>(DaemonStatusParams {}).await.expect("status").connections.len(), 1);
    daemon.stop().await.expect("stop");
}

#[tokio::test]
async fn shutdown_waits_for_idleness_unless_forced() {
    let home = Home::new("daemon-shutdown");
    let (daemon, cli) = start(home.options()).await;
    let gate = tokio_util::sync::CancellationToken::new();
    let g = gate.clone();
    let job = daemon
        .ctx()
        .jobs
        .submit(russetd::jobs::JobSpec::new("test", "busy"), move |_job| async move {
            g.cancelled().await;
            Ok(())
        })
        .expect("job");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let r = cli.call::<DaemonShutdown>(DaemonShutdownParams { force: false, reason: None }).await.expect("answer");
    assert!(!r.accepted, "an active job keeps the daemon");
    assert_eq!(r.active_sessions, 1);
    // An upgrade restart also needs no other connection.
    gate.cancel();
    daemon.ctx().jobs.wait(&job.id, Some(Duration::from_secs(5))).await.expect("wait");
    let (_other, _) = daemon.connect(&client(ClientKind::Ide, "ide", false)).await.expect("ide");
    let r = cli
        .call::<DaemonShutdown>(DaemonShutdownParams { force: false, reason: Some(ShutdownReason::Upgrade) })
        .await
        .expect("answer");
    assert!(!r.accepted, "another client is connected");
    let r = cli.call::<DaemonShutdown>(DaemonShutdownParams { force: true, reason: None }).await.expect("answer");
    assert!(r.accepted);
    tokio::time::timeout(Duration::from_secs(2), daemon.ctx().shutdown.cancelled()).await.expect("shutting down");
    assert_eq!(daemon.ctx().daemon.shutdown_reason(), (ShutdownReason::Request, true));
}

#[tokio::test]
async fn log_filters_change_and_subscribers_get_batches() {
    let home = Home::new("daemon-logs");
    let (daemon, cli) = start(home.options()).await;
    let prev = cli
        .call::<DaemonSetLogFilter>(DaemonSetLogFilterParams { filter: "debug,russetd=trace".into() })
        .await
        .expect("set");
    assert_eq!(prev.previous, "info,idevice=warn,jktcp=warn,hyper=warn");
    let e = cli.call::<DaemonSetLogFilter>(DaemonSetLogFilterParams { filter: "[nope".into() }).await.expect_err("bad");
    assert_eq!(e.code(), codes::core::INVALID_ARGUMENT);

    let batches: Arc<Mutex<Vec<russet_protocol::daemon::DaemonLogBatchParams>>> = Arc::default();
    let b = Arc::clone(&batches);
    cli.on_notification::<DaemonLogBatch, _>(move |p| b.lock().unwrap().push(p));
    let sub = cli
        .call::<DaemonLogsSubscribe>(DaemonLogsSubscribeParams { min_level: Some(TraceLevel::Info) })
        .await
        .expect("subscribe");
    let record = |level, message: &str| DaemonLogRecord {
        ts: russet_core::time::now_rfc3339(),
        level,
        target: "test".into(),
        message: message.into(),
        fields: Default::default(),
    };
    let logs = &daemon.ctx().daemon.logs;
    logs.publish(record(TraceLevel::Debug, "too quiet"));
    logs.publish(record(TraceLevel::Info, "hello"));
    logs.publish(record(TraceLevel::Error, "oops"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    {
        let got = batches.lock().unwrap();
        let messages: Vec<&str> = got.iter().flat_map(|b| b.records.iter().map(|r| r.message.as_str())).collect();
        assert_eq!(messages, vec!["hello", "oops"]);
        assert!(got.iter().all(|b| b.subscription_id == sub.subscription_id));
    }
    cli.call::<SubscriptionCancel>(SubscriptionCancelParams { subscription_id: sub.subscription_id.clone() })
        .await
        .expect("cancel");
    let e = cli
        .call::<SubscriptionCancel>(SubscriptionCancelParams { subscription_id: sub.subscription_id })
        .await
        .expect_err("gone");
    assert_eq!(e.code(), codes::core::NOT_FOUND);
    tokio::time::sleep(Duration::from_millis(50)).await;
    logs.publish(record(TraceLevel::Error, "after cancel"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!batches.lock().unwrap().iter().any(|b| b.records.iter().any(|r| r.message == "after cancel")));
    daemon.stop().await.expect("stop");
}

#[tokio::test]
async fn another_protocol_major_is_refused_and_an_idle_daemon_steps_aside() {
    let home = Home::new("daemon-protocol");
    let (daemon, cli) = start(home.options()).await;
    let mut params = client(ClientKind::Cli, "future", false).initialize_params(None);
    params.protocol.major = 2;
    // Busy (another connection): refused, not restartable.
    let raw = daemon.connect_raw();
    let e = raw.call::<russet_protocol::initialize::Initialize>(params.clone()).await.expect_err("major 2");
    assert_eq!(e.code(), codes::ipc::PROTOCOL_MISMATCH);
    assert_eq!(e.details().and_then(|d| d.get("restartable")), Some(&serde_json::json!(false)));
    tokio::time::timeout(Duration::from_secs(2), raw.closed()).await.expect("the server closes the connection");
    // Idle: restartable, and the daemon shuts down for the newer client.
    cli.close();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let raw = daemon.connect_raw();
    let e = raw.call::<russet_protocol::initialize::Initialize>(params).await.expect_err("major 2");
    assert_eq!(e.details().and_then(|d| d.get("restartable")), Some(&serde_json::json!(true)));
    tokio::time::timeout(Duration::from_secs(2), daemon.ctx().shutdown.cancelled()).await.expect("steps aside");
    assert_eq!(daemon.ctx().daemon.shutdown_reason().0, ShutdownReason::Upgrade);
}
