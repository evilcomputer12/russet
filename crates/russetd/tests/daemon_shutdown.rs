// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Graceful shutdown keeps the endpoint until the jobs have had their grace, then removes the socket and
//! releases the lock last (core §6.3). A client that connects meanwhile gets `ipc.daemonUnavailable` with
//! `details.shuttingDown` from `initialize`; the autostart client waits for the old daemon to go and then
//! reaches the next one, instead of starting a daemon that cannot take `runtime/russetd.lock`.
//!
//! The daemon runs in this test process (`russetd::run`) and stops on SIGTERM, which the test sends to itself:
//! this file holds one test, so no other daemon sees the signal.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use russet_core::rpc::ClientKind;
use russet_ipc::{CallOptions, Endpoint, Router, SpawnSpec};
use russet_protocol::daemon::{DaemonShutdown, DaemonShutdownParams, ShutdownReason};
use russetd::DaemonCtx;
use russetd::daemon::lock::FileLock;
use serde_json::{Value, json};
use support::{Home, client};

russet_protocol::rpc_method!(
    /// A job that ignores cancellation for 1.5 s, like a build step finishing its file.
    TestSlow,
    "test.slow",
    Value => Value,
    Permission::Build,
    long_running
);

fn options(home: &Home) -> russetd::DaemonOptions {
    let mut o = home.options();
    o.idle_timeout = None;
    o.registrations.push(Arc::new(|r: &mut Router<DaemonCtx>| {
        r.request::<TestSlow, _, _>(|_cx, _rq, _p: Value| async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            Ok(json!("done"))
        });
    }));
    o
}

async fn connect_when_up(endpoint: &Endpoint) -> (russet_ipc::Peer, russet_protocol::initialize::InitializeResult) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match russet_ipc::connect(endpoint, client(ClientKind::Cli, "test-cli", true)).await {
            Ok(ok) => return ok,
            Err(e) if Instant::now() < deadline => {
                let _ = e;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("the daemon did not come up: {e:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_arriving_during_shutdown_waits_and_reaches_the_next_daemon() {
    let home = Home::new("shutdown-order");
    let endpoint = Endpoint::default_for(&home.dirs);
    let first = tokio::spawn(russetd::run(options(&home)));
    let (worker, first_init) = connect_when_up(&endpoint).await;
    let job = tokio::spawn(async move { worker.call_raw("test.slow", json!({}), CallOptions::default()).await });
    // Let the signal watcher register and the job start.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let kill = std::process::Command::new("kill").args(["-TERM", &std::process::id().to_string()]).status();
    assert!(kill.expect("kill runs").success());
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The endpoint is still there while the job has its grace; initialize says why it cannot be used.
    let e = russet_ipc::connect(&endpoint, client(ClientKind::Cli, "late", true)).await.expect_err("stopping");
    assert!(russet_ipc::client::is_shutting_down(&e), "{e:?}");
    assert_eq!(e.details().and_then(|d| d.get("graceMs")).and_then(Value::as_u64), Some(10_000));
    assert!(e.is_retryable());
    assert!(FileLock::try_take(&home.dirs.daemon_lock()).unwrap().is_none(), "the lock is released last");

    // An autostarting client waits for the old daemon instead of failing; `/bin/true` stands in for the spawn.
    let waiting = {
        let dirs = home.dirs.clone();
        tokio::spawn(async move {
            let spec = SpawnSpec {
                executable: "/bin/true".into(),
                args: Vec::new(),
                ready_timeout: Duration::from_secs(10),
                expect: None,
            };
            russet_ipc::connect_or_spawn(&dirs, client(ClientKind::Cli, "autostart", true), spec).await
        })
    };
    let stopped_at = {
        first.await.expect("join").expect("the first daemon stops cleanly");
        Instant::now()
    };
    let _ = job.await.expect("join");
    assert!(endpoint.open(Duration::from_millis(200)).await.is_err(), "the socket is gone");
    drop(FileLock::try_take(&home.dirs.daemon_lock()).unwrap().expect("the lock is free"));
    assert!(!waiting.is_finished(), "the client is still waiting for a daemon");

    // The next daemon (what the client's spawn starts) comes up; the waiting client reaches it.
    let second = tokio::spawn(russetd::run(options(&home)));
    let (peer, second_init) = waiting.await.expect("join").expect("the waiting client reaches the next daemon");
    assert_ne!(second_init.server.started_at, first_init.server.started_at);
    assert!(stopped_at.elapsed() < Duration::from_secs(10));
    let r = peer
        .call::<DaemonShutdown>(DaemonShutdownParams { force: false, reason: Some(ShutdownReason::Request) })
        .await
        .expect("shutdown");
    assert!(r.accepted);
    second.await.expect("join").expect("the second daemon stops cleanly");
}
