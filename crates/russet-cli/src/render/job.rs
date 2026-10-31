// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Long-running calls as every area command runs them (ARCHITECTURE §7.6; `russet build`, `install`, `run`,
//! `debug`, …): use [`crate::CliCtx::call_long`].
//!
//! - `$/progress` goes to the reporter (a spinner on a TTY; nothing with `--json`).
//! - The first interrupt (Ctrl-C) sends `$/cancelRequest`: the daemon cancels the job, the call fails with
//!   `core.cancelled` and the CLI exits 130 (ARCHITECTURE §13.4). A second interrupt gives up waiting for the
//!   daemon and fails with `core.cancelled` at once.
//! - With `logs`, the job's `log` events are printed as they arrive: the `begin` progress names the job, the CLI
//!   subscribes to it (`job.subscribe {jobId, includeEvents}`), backfills what it missed from `job.log` and,
//!   once the result is in, prints the rest. Every line is printed once, in `seq` order.
//! - `background: true` in the params makes the daemon answer `{jobId}` at once (§7.6); the helper returns it
//!   like any result.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use russet_core::CancellationToken;
use russet_core::error::{Error, Result};
use russet_core::ids::{JobId, SubscriptionId};
use russet_core::progress::JobEventKind;
use russet_core::rpc::Method;
use russet_ipc::{CallOptions, Peer, ProgressValue};
use russet_protocol::common::ProgressKind;
use russet_protocol::job::{JobEvent, JobEvents, JobLog, JobLogParams, JobSubscribe, JobSubscribeParams};
use russet_protocol::subscription::{SubscriptionCancel, SubscriptionCancelParams};

use super::Reporter;

/// The job whose log lines are being printed.
struct Follow {
    job: JobId,
    subscription: Option<SubscriptionId>,
    /// The highest `seq` printed so far.
    printed: u64,
}

impl Follow {
    fn print(&mut self, reporter: &dyn Reporter, events: &[JobEvent]) {
        for e in events {
            if e.seq <= self.printed {
                continue;
            }
            self.printed = e.seq;
            if let JobEventKind::Log { stream, line } = &e.event {
                reporter.log(*stream, line);
            }
        }
    }

    /// Prints the job's log events after the last printed one.
    async fn backfill(&mut self, peer: &Peer, reporter: &dyn Reporter) {
        let params =
            JobLogParams { job_id: self.job.clone(), after_seq: Some(self.printed), tail: None, kinds: Vec::new() };
        if let Ok(log) = peer.call::<JobLog>(params).await {
            self.print(reporter, &log.events);
        }
    }
}

/// Calls the long-running method `M` (module docs). `interrupt` makes the future of the next interrupt
/// (`tokio::signal::ctrl_c` in the CLI).
pub async fn call_long<M, I, F>(
    peer: &Peer,
    reporter: Arc<dyn Reporter>,
    params: M::Params,
    logs: bool,
    mut interrupt: I,
) -> Result<M::Result>
where
    M: Method,
    I: FnMut() -> F,
    F: Future<Output = ()>,
{
    let cancel = CancellationToken::new();
    let (job_tx, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<JobId>();
    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
    if logs {
        peer.on_notification::<JobEvents, _>(move |batch| {
            let _ = events_tx.send(batch);
        });
    }
    let progress_reporter = Arc::clone(&reporter);
    let opts = CallOptions {
        cancel: Some(cancel.clone()),
        on_progress: Some(Box::new(move |p: ProgressValue| {
            if let (ProgressKind::Begin, Some(id)) = (p.kind, &p.job_id) {
                let _ = job_tx.send(id.clone());
            }
            progress_reporter.progress(&p);
        })),
        timeout: None,
    };
    let call = peer.call_with::<M>(params, opts);
    tokio::pin!(call);
    let mut next_interrupt: Pin<Box<F>> = Box::pin(interrupt());
    let mut follow: Option<Follow> = None;
    let result = loop {
        tokio::select! {
            r = &mut call => break r,
            () = &mut next_interrupt => {
                if cancel.is_cancelled() {
                    // The daemon did not answer the cancellation: stop waiting (the job ends when this
                    // connection closes, unless it runs in the background).
                    break Err(Error::cancelled().with_hint("Interrupted twice; the CLI stopped waiting for the daemon."));
                }
                cancel.cancel();
                next_interrupt = Box::pin(interrupt());
            }
            Some(job) = job_rx.recv(), if logs && follow.is_none() => {
                let subscription = peer
                    .call::<JobSubscribe>(JobSubscribeParams {
                        job_id: Some(job.clone()),
                        kinds: Vec::new(),
                        include_events: true,
                    })
                    .await
                    .ok()
                    .map(|r| r.subscription_id);
                let mut f = Follow { job, subscription, printed: 0 };
                f.backfill(peer, reporter.as_ref()).await;
                follow = Some(f);
            }
            Some(batch) = events_rx.recv() => {
                if let Some(f) = follow.as_mut().filter(|f| f.job == batch.job_id) {
                    f.print(reporter.as_ref(), &batch.events);
                }
            }
        }
    };
    if let Some(mut f) = follow {
        // job.events are batched: the last lines may still be on their way when the result arrives.
        while let Ok(batch) = events_rx.try_recv() {
            if batch.job_id == f.job {
                f.print(reporter.as_ref(), &batch.events);
            }
        }
        if !peer.is_closed() {
            f.backfill(peer, reporter.as_ref()).await;
            if let Some(id) = f.subscription.take() {
                let _ = peer.call::<SubscriptionCancel>(SubscriptionCancelParams { subscription_id: id }).await;
            }
        }
    }
    result
}
