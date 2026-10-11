// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `job.subscribe` and `subscription.cancel` (ARCHITECTURE §6.4, §9.1): `job.updated` for every state change,
//! `job.events` batches with `includeEvents`, filters by job, and nothing after the cancel.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_core::progress::{EventSink, LogStream, ProgressSink};
use russet_protocol::job::{
    JobEvents, JobEventsParams, JobLog, JobLogParams, JobState, JobSubscribe, JobSubscribeParams, JobUpdated,
    JobUpdatedParams,
};
use russet_protocol::subscription::{SubscriptionCancel, SubscriptionCancelParams};
use russetd::jobs::JobSpec;
use support::{Home, start};

#[tokio::test]
async fn subscribers_see_states_and_events_until_they_cancel() {
    let home = Home::new("sub-jobs");
    let (daemon, cli) = start(home.options()).await;
    let updates: Arc<Mutex<Vec<JobUpdatedParams>>> = Arc::default();
    let events: Arc<Mutex<Vec<JobEventsParams>>> = Arc::default();
    let (u, e) = (Arc::clone(&updates), Arc::clone(&events));
    cli.on_notification::<JobUpdated, _>(move |p| u.lock().unwrap().push(p));
    cli.on_notification::<JobEvents, _>(move |p| e.lock().unwrap().push(p));
    let sub = cli
        .call::<JobSubscribe>(JobSubscribeParams { job_id: None, kinds: vec!["build".into()], include_events: true })
        .await
        .expect("subscribe");

    let run = |kind: &str| {
        daemon
            .ctx()
            .jobs
            .submit(JobSpec::new(kind, "demo"), |job| async move {
                job.events.begin("compiling", true);
                for i in 0..3 {
                    job.events.log(LogStream::Stdout, &format!("line {i}"));
                }
                job.events.emit(russet_core::progress::JobEventKind::Stage {
                    stage: russet_core::progress::Stage::Compile,
                    state: russet_core::progress::StepState::Finished,
                });
                Ok(())
            })
            .expect("job")
            .id
    };
    let build = run("build");
    let other = run("install");
    daemon.ctx().jobs.wait(&build, Some(Duration::from_secs(5))).await.expect("wait");
    daemon.ctx().jobs.wait(&other, Some(Duration::from_secs(5))).await.expect("wait");
    tokio::time::sleep(Duration::from_millis(300)).await;
    {
        let updates = updates.lock().unwrap();
        let states: Vec<JobState> = updates.iter().filter(|u| u.job.job_id == build).map(|u| u.job.state).collect();
        assert_eq!(states, vec![JobState::Queued, JobState::Running, JobState::Succeeded]);
        assert!(updates.iter().all(|u| u.job.job_id != other), "kinds filter");
        assert!(updates.iter().all(|u| u.subscription_id == sub.subscription_id));
        let events = events.lock().unwrap();
        let seqs: Vec<u64> =
            events.iter().filter(|b| b.job_id == build).flat_map(|b| b.events.iter().map(|e| e.seq)).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5], "in order, none lost");
    }
    // job.log has the same events; tail and kinds filter them.
    let log = cli
        .call::<JobLog>(JobLogParams {
            job_id: build.clone(),
            after_seq: None,
            tail: Some(2),
            kinds: vec!["log".into()],
        })
        .await
        .expect("log");
    assert_eq!(log.events.len(), 2);
    assert_eq!(log.next_seq, 4);

    cli.call::<SubscriptionCancel>(SubscriptionCancelParams { subscription_id: sub.subscription_id })
        .await
        .expect("cancel");
    let before = updates.lock().unwrap().len();
    let later = run("build");
    daemon.ctx().jobs.wait(&later, Some(Duration::from_secs(5))).await.expect("wait");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(updates.lock().unwrap().len(), before, "nothing after the cancel");
    daemon.stop().await.expect("stop");
}
