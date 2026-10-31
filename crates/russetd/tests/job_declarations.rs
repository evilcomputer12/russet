// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Per-method job declarations (ARCHITECTURE §6.2) over RPC: an area declares, from its `register`, what a
//! long-running method's job locks; two calls on one project queue behind the project lock while a call on
//! another project runs, the declared kind, title and event mirror apply, and a declaration that fails answers
//! the call without a job. `JobManager::lock` gives code outside jobs the same locks.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_core::error::{Error, codes};
use russet_core::progress::{LogStream, ProgressSink};
use russet_ipc::{CallOptions, Router};
use russet_protocol::job::{JobInfo, JobListParams, JobState};
use russetd::jobs::{JobOptions, JobSpec, LockKey, declare};
use russetd::{DaemonCtx, JobLimits};
use serde_json::{Value, json};
use support::{Home, start};
use tokio_util::sync::CancellationToken;

russet_protocol::rpc_method!(
    /// A test build: waits until its `gate` opens, then answers.
    TestBuild,
    "test.build",
    Value => Value,
    Permission::Build,
    long_running
);

russet_protocol::rpc_method!(
    /// A test install whose declaration cannot resolve its device.
    TestInstall,
    "test.install",
    Value => Value,
    Permission::Device,
    long_running
);

/// Named gates the test opens one by one.
#[derive(Default)]
struct Gates(Mutex<BTreeMap<String, CancellationToken>>);

impl Gates {
    fn gate(&self, name: &str) -> CancellationToken {
        self.0.lock().unwrap().entry(name.to_owned()).or_default().clone()
    }
}

fn register(gates: Arc<Gates>) -> russetd::Registration {
    Arc::new(move |r: &mut Router<DaemonCtx>| {
        let g = Arc::clone(&gates);
        r.request::<TestBuild, _, _>(move |_cx, rq: russet_ipc::RequestCtx, p: Value| {
            let gate = g.gate(p["gate"].as_str().unwrap_or_default());
            async move {
                rq.progress.log(LogStream::Stdout, &format!("compiling behind gate {}", p["gate"]));
                gate.cancelled().await;
                Ok(json!({"gate": p["gate"]}))
            }
        });
        declare::<TestBuild, _, _>(r, |_cx, p: Value| async move {
            let project = PathBuf::from(p["project"].as_str().unwrap_or("/nowhere"));
            Ok(JobOptions::build_of(&project).kind("build").title(format!("Build {}", project.display())))
        });
        r.request::<TestInstall, _, _>(|_cx, _rq, _p: Value| async move { Ok(json!({})) });
        declare::<TestInstall, _, _>(r, |_cx, p: Value| async move {
            Err::<JobOptions, _>(
                Error::new(codes::core::NOT_FOUND, format!("no device matches {}", p["device"]))
                    .with_hint("Connect it, or pass another --device."),
            )
        });
    })
}

fn job_of(jobs: &[JobInfo], title: &str) -> Option<JobInfo> {
    jobs.iter().find(|j| j.title == title).cloned()
}

async fn wait_for(cx: &DaemonCtx, mut ok: impl FnMut(&[JobInfo]) -> bool) -> Vec<JobInfo> {
    for _ in 0..200 {
        let jobs = cx.jobs.list(&JobListParams::default());
        if ok(&jobs) {
            return jobs;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("jobs never reached the expected state: {:?}", cx.jobs.list(&JobListParams::default()));
}

#[tokio::test]
async fn two_calls_on_one_project_queue_behind_its_lock() {
    let home = Home::new("job-declarations");
    let gates = Arc::new(Gates::default());
    let mut o = home.options();
    // Only the project lock may serialize these builds, not the build slot.
    o.limits = JobLimits { max_parallel: 4, max_heavy: 4 };
    o.registrations.push(register(Arc::clone(&gates)));
    let (daemon, cli) = start(o).await;
    let app = home.dir.path().join("App");
    let other = home.dir.path().join("Other");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let call = |project: &Path, gate: &str| {
        let (cli, params) = (cli.clone(), json!({"project": project.display().to_string(), "gate": gate}));
        tokio::spawn(async move { cli.call_raw("test.build", params, CallOptions::default()).await })
    };
    let title = |p: &Path| format!("Build {}", p.display());

    let first = call(&app, "a");
    wait_for(daemon.ctx(), |j| job_of(j, &title(&app)).is_some_and(|j| j.state == JobState::Running)).await;
    let second = call(&app, "b");
    let third = call(&other, "c");
    let jobs = wait_for(daemon.ctx(), |j| {
        j.iter().filter(|j| j.title == title(&app)).count() == 2
            && job_of(j, &title(&other)).is_some_and(|j| j.state == JobState::Running)
    })
    .await;
    let queued: Vec<&JobInfo> = jobs.iter().filter(|j| j.title == title(&app) && j.state == JobState::Queued).collect();
    assert_eq!(queued.len(), 1, "{jobs:?}");
    assert!(queued[0].waiting_for.as_deref().is_some_and(|w| w.contains("project lock")), "{:?}", queued[0]);
    assert!(jobs.iter().all(|j| j.kind == "build"), "the declared kind: {jobs:?}");
    assert_eq!(
        jobs.iter().find(|j| j.title == title(&other)).and_then(|j| j.project.clone()),
        Some(other.display().to_string())
    );

    // The first build ends; the queued one takes the lock.
    gates.gate("a").cancel();
    assert_eq!(first.await.unwrap().expect("first")["gate"], "a");
    wait_for(daemon.ctx(), |j| {
        j.iter().filter(|j| j.title == title(&app) && j.state == JobState::Running).count() == 1
    })
    .await;
    gates.gate("b").cancel();
    gates.gate("c").cancel();
    assert_eq!(second.await.unwrap().expect("second")["gate"], "b");
    assert_eq!(third.await.unwrap().expect("third")["gate"], "c");

    // Build jobs mirror their events to <project>/.russet/logs/<job-id>.jsonl.
    let logs = app.join(".russet/logs");
    let mirrored: Vec<String> = std::fs::read_dir(&logs)
        .expect("the mirror directory")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let done = daemon.ctx().jobs.list(&JobListParams::default());
    for job in done.iter().filter(|j| j.title == title(&app)) {
        assert_eq!(job.state, JobState::Succeeded);
        assert!(mirrored.contains(&format!("{}.jsonl", job.job_id)), "{mirrored:?}");
    }
    let lines = std::fs::read_to_string(logs.join(&mirrored[0])).unwrap();
    assert!(lines.contains("compiling behind gate"), "the handler's log line is mirrored: {lines}");
    daemon.stop().await.expect("stop");
}

#[tokio::test]
async fn a_failed_declaration_answers_the_call_without_a_job() {
    let home = Home::new("job-declaration-error");
    let mut o = home.options();
    o.registrations.push(register(Arc::new(Gates::default())));
    let (daemon, cli) = start(o).await;
    let e = cli
        .call_raw("test.install", json!({"device": "iphone"}), CallOptions::default())
        .await
        .expect_err("no device")
        .to_error();
    assert_eq!(e.code(), codes::core::NOT_FOUND);
    assert!(e.message().contains("iphone"), "{}", e.message());
    assert!(daemon.ctx().jobs.list(&JobListParams::default()).is_empty());
    daemon.stop().await.expect("stop");
}

#[tokio::test]
async fn code_outside_jobs_takes_the_same_locks() {
    let home = Home::new("job-locks");
    let (daemon, _cli) = start(home.options()).await;
    let jobs = daemon.ctx().jobs.clone();
    let never = CancellationToken::new();
    // app.uninstall (not a job) holds the device lock; an install job of the same device waits for it.
    let held = jobs.lock(vec![LockKey::Device("u1".into())], &never).await.expect("free");
    let mut spec = JobSpec::new("install", "install on u1");
    spec.locks = vec![LockKey::Device("u1".into())];
    let id = jobs.submit(spec, |_job| async move { Ok(json!("installed")) }).expect("submit").id;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let info = jobs.get(&id).unwrap();
    assert_eq!(info.state, JobState::Queued);
    assert!(info.waiting_for.as_deref().is_some_and(|w| w.contains("device lock")), "{info:?}");
    // A second taker waits too, and gives up when cancelled.
    let cancel = CancellationToken::new();
    let waiter = {
        let (jobs, cancel) = (jobs.clone(), cancel.clone());
        tokio::spawn(async move { jobs.lock(vec![LockKey::Device("u1".into())], &cancel).await.map(|_| ()) })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    assert_eq!(waiter.await.unwrap().expect_err("cancelled").code(), codes::core::CANCELLED);
    drop(held);
    let r = jobs.wait(&id, Some(Duration::from_secs(5))).await.unwrap();
    assert_eq!(r.job.state, JobState::Succeeded);
    // Other devices were never blocked.
    let other = jobs.lock(vec![LockKey::Device("u2".into())], &never).await.expect("free");
    assert_eq!(other.keys(), [LockKey::Device("u2".into())]);
    daemon.stop().await.expect("stop");
}
