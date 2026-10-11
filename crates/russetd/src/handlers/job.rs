// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `job.*` handlers (ARCHITECTURE §6.2, §7.6, §8.1, §9.1). Owner: W2.
//!
//! `job.subscribe` streams `job.updated` (every state change) and, with `includeEvents`, `job.events` batched
//! for at most 50 ms per job, through a bounded queue that drops the oldest messages for a slow consumer
//! (ARCHITECTURE §6.4).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use russet_core::error::{Error, Result};
use russet_core::ids::JobId;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::job::{
    self as p, JobCancelParams, JobCancelResult, JobEvent, JobEvents, JobEventsParams, JobGetParams, JobInfo,
    JobListParams, JobListResult, JobLogParams, JobLogResult, JobSubscribeParams, JobSubscribeResult, JobUpdated,
    JobUpdatedParams, JobWaitParams, JobWaitResult,
};
use tokio::sync::broadcast::error::RecvError;

use crate::DaemonCtx;
use crate::jobs::JobNotice;
use crate::subs::{BATCH_WINDOW, DropQueue, MESSAGE_QUEUE, wait_for_room};

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::JobList, _, _>(list);
    r.request::<p::JobGet, _, _>(get);
    r.request::<p::JobWait, _, _>(wait);
    r.request::<p::JobCancel, _, _>(cancel);
    r.request::<p::JobSubscribe, _, _>(subscribe);
    r.request::<p::JobLog, _, _>(log);
}

async fn list(cx: DaemonCtx, _rq: RequestCtx, p: JobListParams) -> Result<JobListResult> {
    Ok(JobListResult { jobs: cx.jobs.list(&p) })
}

async fn get(cx: DaemonCtx, _rq: RequestCtx, p: JobGetParams) -> Result<JobInfo> {
    cx.jobs.get(&p.job_id)
}

async fn wait(cx: DaemonCtx, rq: RequestCtx, p: JobWaitParams) -> Result<JobWaitResult> {
    tokio::select! {
        r = cx.jobs.wait(&p.job_id, p.timeout_ms.map(Duration::from_millis)) => r,
        () = rq.cancel.cancelled() => Err(Error::cancelled()),
    }
}

async fn cancel(cx: DaemonCtx, _rq: RequestCtx, p: JobCancelParams) -> Result<JobCancelResult> {
    Ok(JobCancelResult { state: cx.jobs.cancel(&p.job_id)? })
}

async fn log(cx: DaemonCtx, _rq: RequestCtx, p: JobLogParams) -> Result<JobLogResult> {
    cx.jobs.log(&p)
}

/// One queued message of a job subscription.
#[derive(Debug)]
enum Message {
    Updated(Box<JobInfo>),
    Event(JobId, JobEvent),
}

async fn subscribe(cx: DaemonCtx, rq: RequestCtx, p: JobSubscribeParams) -> Result<JobSubscribeResult> {
    if let Some(id) = &p.job_id {
        cx.jobs.get(id)?;
    }
    let sub = cx.subs.open(&rq.peer, "job");
    let mut rx = cx.jobs.subscribe();
    let queue: Arc<DropQueue<Message>> = Arc::new(DropQueue::new(MESSAGE_QUEUE));
    let (peer, id, cancel, jobs) = (rq.peer.clone(), sub.id.clone(), sub.cancel.clone(), cx.jobs.clone());
    tokio::spawn(async move {
        // Job kinds seen so far, for the `kinds` filter of events.
        let mut kinds: BTreeMap<JobId, String> = BTreeMap::new();
        let wanted = |info_kind: &str, job: &JobId| {
            p.job_id.as_ref().is_none_or(|want| want == job)
                && (p.kinds.is_empty() || p.kinds.iter().any(|k| k == info_kind))
        };
        loop {
            let window = tokio::time::sleep(BATCH_WINDOW);
            tokio::pin!(window);
            let mut got_any = false;
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = &mut window, if got_any => break,
                    r = rx.recv() => {
                        got_any = true;
                        match r {
                            Ok(JobNotice::Updated(info)) => {
                                kinds.insert(info.job_id.clone(), info.kind.clone());
                                if wanted(&info.kind, &info.job_id) {
                                    queue.push(Message::Updated(info));
                                }
                            }
                            Ok(JobNotice::Event { job_id, event }) => {
                                if !p.include_events {
                                    continue;
                                }
                                let kind = match kinds.get(&job_id) {
                                    Some(k) => k.clone(),
                                    None => {
                                        let k = jobs.get(&job_id).map(|i| i.kind).unwrap_or_default();
                                        kinds.insert(job_id.clone(), k.clone());
                                        k
                                    }
                                };
                                if wanted(&kind, &job_id) {
                                    queue.push(Message::Event(job_id, event));
                                }
                            }
                            Err(RecvError::Lagged(n)) => queue.note_dropped(n),
                            Err(RecvError::Closed) => return,
                        }
                    }
                }
            }
            if !wait_for_room(&peer, &cancel).await {
                return;
            }
            let (messages, dropped) = queue.take(MESSAGE_QUEUE);
            let mut dropped = (dropped > 0).then_some(dropped);
            // Events of one job go out as one batch, in order; updates go out one by one.
            let mut events: BTreeMap<JobId, Vec<JobEvent>> = BTreeMap::new();
            for m in messages {
                match m {
                    Message::Updated(job) => {
                        if peer.notify::<JobUpdated>(JobUpdatedParams { subscription_id: id.clone(), job: *job }).is_err() {
                            return;
                        }
                    }
                    Message::Event(job, event) => events.entry(job).or_default().push(event),
                }
            }
            for (job_id, events) in events {
                let params = JobEventsParams { subscription_id: id.clone(), job_id, events, dropped: dropped.take() };
                if peer.notify::<JobEvents>(params).is_err() {
                    return;
                }
            }
        }
    });
    Ok(JobSubscribeResult { subscription_id: sub.id })
}
