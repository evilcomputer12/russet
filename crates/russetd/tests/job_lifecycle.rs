// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Jobs (ARCHITECTURE §6.2, §7.6; PLAN W2 tests): a non-background job is cancelled within 1 s of its owner's
//! disconnect and its grandchild process dies within 2 s; a `background` job survives; limits, locks,
//! persistence and crash recovery; long-running methods run as jobs and `background: true` returns `{jobId}`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use std::time::{Duration, Instant};

use russet_core::error::codes;
use russet_core::proc::{CommandRunner, CommandSpec, ProcessRunner};
use russet_core::progress::EventSink;
use russet_core::rpc::ClientKind;
use russet_ipc::CallOptions;
use russet_protocol::common::ConnectionRef;
use russet_protocol::job::{JobGet, JobGetParams, JobListParams, JobLogParams, JobState, JobWait, JobWaitParams};
use russetd::jobs::{JobLimits, JobManager, JobOwner, JobSpec, LockKey};
use serde_json::json;
use support::{Home, client, start};
use tokio_util::sync::CancellationToken;

/// Whether `pid` runs and is not a zombie.
#[cfg(target_os = "linux")]
fn running(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            !matches!(stat.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().next()), Some("Z" | "X") | None)
        }
        Err(_) => false,
    }
}

/// A job that starts `sh -c 'sleep 30 & echo $!; wait'` and runs until cancelled.
fn grandchild_job(mgr: &JobManager, owner: Option<JobOwner>, background: bool) -> russet_core::ids::JobId {
    let mut spec = JobSpec::new("test.sleep", "sleep with a grandchild");
    spec.owner = owner;
    spec.background = background;
    mgr.submit(spec, |job| async move {
        let spec = CommandSpec::new("/bin/sh").arg("-c").arg("sleep 30 & echo $!; wait");
        ProcessRunner.run(&spec, job.events.as_ref(), &job.cancel).await.map(|_| ())
    })
    .expect("submitted")
    .id
}

/// The grandchild's pid, from the job's first stdout line.
async fn grandchild_pid(mgr: &JobManager, id: &russet_core::ids::JobId) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let log = mgr
            .log(&JobLogParams { job_id: id.clone(), after_seq: None, tail: None, kinds: vec!["log".into()] })
            .expect("log");
        if let Some(pid) = log.events.iter().find_map(|e| match &e.event {
            russet_core::progress::JobEventKind::Log { line, .. } => line.trim().parse::<u32>().ok(),
            _ => None,
        }) {
            return pid;
        }
        assert!(Instant::now() < deadline, "the job never printed its grandchild's pid");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn owner_of(peer: &russet_ipc::Peer) -> JobOwner {
    JobOwner {
        conn: ConnectionRef {
            kind: ClientKind::Cli,
            name: "owner".into(),
            connection_id: peer.connection_id().to_owned(),
        },
        closed: peer.closed_token(),
    }
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disconnect_cancels_the_job_within_1s_and_kills_its_grandchild_within_2s() {
    let home = Home::new("job-disconnect");
    let mgr = JobManager::new(Some(home.dirs.jobs()), JobLimits::default(), CancellationToken::new());
    let (client_peer, server_peer) = russet_ipc::in_process();
    let id = grandchild_job(&mgr, Some(owner_of(&server_peer)), false);
    let pid = grandchild_pid(&mgr, &id).await;
    assert!(running(pid));
    assert_eq!(mgr.get(&id).unwrap().state, JobState::Running);

    // The requester disconnects.
    client_peer.close();
    let gone = Instant::now();
    let waited = mgr.wait(&id, Some(Duration::from_secs(1))).await.expect("wait");
    assert_eq!(waited.job.state, JobState::Cancelled, "cancelled within 1 s of the disconnect");
    assert!(gone.elapsed() <= Duration::from_secs(1), "{:?}", gone.elapsed());
    while running(pid) {
        assert!(gone.elapsed() < Duration::from_secs(2), "grandchild {pid} outlived 2 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_job_survives_its_owner() {
    let home = Home::new("job-background");
    let mgr = JobManager::new(Some(home.dirs.jobs()), JobLimits::default(), CancellationToken::new());
    let (client_peer, server_peer) = russet_ipc::in_process();
    let id = grandchild_job(&mgr, Some(owner_of(&server_peer)), true);
    let pid = grandchild_pid(&mgr, &id).await;
    client_peer.close();
    tokio::time::timeout(Duration::from_secs(2), server_peer.closed()).await.expect("disconnected");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(mgr.get(&id).unwrap().state, JobState::Running, "background jobs outlive their requester");
    assert!(running(pid));
    assert_eq!(mgr.cancel(&id).unwrap(), JobState::Cancelled);
    assert_eq!(mgr.wait(&id, Some(Duration::from_secs(5))).await.unwrap().job.state, JobState::Cancelled);
}

#[tokio::test]
async fn limits_and_locks_queue_jobs() {
    let home = Home::new("job-limits");
    let mgr =
        JobManager::new(Some(home.dirs.jobs()), JobLimits { max_parallel: 4, max_heavy: 1 }, CancellationToken::new());
    let gate = CancellationToken::new();
    let hold = |mgr: &JobManager, spec: JobSpec, gate: CancellationToken| {
        mgr.submit(spec, move |job| async move {
            tokio::select! {
                () = gate.cancelled() => Ok(json!("done")),
                () = job.cancel.cancelled() => Err(russet_core::Error::cancelled()),
            }
        })
        .expect("submitted")
        .id
    };
    let mut a = JobSpec::new("build", "first build");
    a.heavy = true;
    a.locks = vec![LockKey::Project("/p".into())];
    let mut b = JobSpec::new("build", "second build");
    b.heavy = true;
    let mut c = JobSpec::new("install", "same project lock");
    c.locks = vec![LockKey::Project("/p".into())];
    let first = hold(&mgr, a, gate.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;
    let second = hold(&mgr, b, gate.clone());
    let third = hold(&mgr, c, gate.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(mgr.get(&first).unwrap().state, JobState::Running);
    let b = mgr.get(&second).unwrap();
    assert_eq!(b.state, JobState::Queued);
    assert!(b.waiting_for.as_deref().is_some_and(|w| w.contains("build slot")), "{b:?}");
    let c = mgr.get(&third).unwrap();
    assert_eq!(c.state, JobState::Queued);
    assert!(c.waiting_for.as_deref().is_some_and(|w| w.contains("project lock")), "{c:?}");
    assert_eq!(mgr.counts().running, 1);
    gate.cancel();
    for id in [&first, &second, &third] {
        let r = mgr.wait(id, Some(Duration::from_secs(5))).await.unwrap();
        assert_eq!(r.job.state, JobState::Succeeded);
        assert_eq!(r.result, Some(json!("done")));
    }
    let listed = mgr.list(&JobListParams { kinds: vec!["build".into()], ..JobListParams::default() });
    assert_eq!(listed.len(), 2);
}

#[tokio::test]
async fn jobs_persist_and_a_crash_leaves_them_interrupted() {
    let home = Home::new("job-recovery");
    let root = home.dirs.jobs();
    let mgr = JobManager::new(Some(root.clone()), JobLimits::default(), CancellationToken::new());
    let done = mgr
        .submit(JobSpec::new("test", "quick"), |job| async move {
            job.events.emit(russet_core::progress::JobEventKind::Custom { name: "x".into(), data: json!(1) });
            Ok(json!({"answer": 42}))
        })
        .expect("submit")
        .id;
    mgr.wait(&done, None).await.unwrap();
    let stuck = mgr
        .submit(JobSpec::new("test", "never ends"), |job| async move {
            job.cancel.cancelled().await;
            Ok(())
        })
        .expect("submit")
        .id;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(root.join(done.as_str()).join("result.json").exists());
    assert!(root.join(done.as_str()).join("events.ndjson").exists());

    // A new daemon on the same state: the finished job keeps its result; the running one was interrupted.
    let again = JobManager::new(Some(root.clone()), JobLimits::default(), CancellationToken::new());
    let r = again.wait(&done, None).await.unwrap();
    assert_eq!(r.job.state, JobState::Succeeded);
    assert_eq!(r.result, Some(json!({"answer": 42})));
    let log = again.log(&JobLogParams { job_id: done.clone(), after_seq: None, tail: None, kinds: vec![] }).unwrap();
    assert_eq!(log.events.len(), 1);
    assert_eq!(log.next_seq, 1);
    let s = again.get(&stuck).unwrap();
    assert_eq!(s.state, JobState::Interrupted);
    assert_eq!(s.error.map(|e| e.russet_code), Some("core.internal".to_owned()));
    mgr.cancel(&stuck).unwrap();
}

#[tokio::test]
async fn long_running_methods_are_jobs_and_background_returns_the_job_id() {
    let home = Home::new("job-layer");
    let (daemon, cli) = start(home.options()).await;
    // toolchain.install is long-running (W3 lands its body): its call runs as a job that fails with
    // core.notImplemented, and the request waits for it.
    let e = cli
        .call_raw("toolchain.install", json!({"component": "darwin-tools", "version": "1.1.0"}), CallOptions::default())
        .await
        .expect_err("not implemented yet");
    assert_eq!(e.to_error().code(), codes::core::NOT_IMPLEMENTED);
    let jobs = daemon.ctx().jobs.list(&JobListParams::default());
    assert_eq!(jobs.len(), 1);
    assert_eq!((jobs[0].kind.as_str(), jobs[0].state), ("toolchain.install", JobState::Failed));
    assert_eq!(jobs[0].owner.as_ref().map(|o| o.name.as_str()), Some("test-cli"));

    // background: true returns {jobId} at once; job.wait gets the outcome.
    let v = cli
        .call_raw(
            "toolchain.install",
            json!({"component": "darwin-tools", "version": "1.1.0", "background": true}),
            CallOptions::default(),
        )
        .await
        .expect("returns at once");
    let id = russet_core::ids::JobId::parse(v["jobId"].as_str().expect("jobId")).expect("a job id");
    let waited = cli.call::<JobWait>(JobWaitParams { job_id: id.clone(), timeout_ms: Some(5000) }).await.expect("wait");
    assert_eq!(waited.job.state, JobState::Failed);
    assert_eq!(waited.error.map(|e| e.russet_code), Some("core.notImplemented".to_owned()));
    let got = cli.call::<JobGet>(JobGetParams { job_id: id }).await.expect("get");
    assert!(got.background);

    // An MCP client may not cancel another connection's job (system), its owner may (read).
    let mcp = daemon.connect(&client(ClientKind::Mcp, "agent", false)).await.expect("mcp").0;
    let e = mcp
        .call_raw("job.cancel", json!({"jobId": got.job_id}), CallOptions::default())
        .await
        .expect_err("not its job");
    assert_eq!(e.to_error().code(), codes::perm::DENIED);
    cli.call_raw("job.cancel", json!({"jobId": got.job_id}), CallOptions::default()).await.expect("owner cancels");
    daemon.stop().await.expect("stop");
}
