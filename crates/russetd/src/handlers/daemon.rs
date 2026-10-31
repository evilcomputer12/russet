// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `daemon.*` handlers (ARCHITECTURE §8.1). Owner: W2.

use std::sync::Arc;
use std::time::Duration;

use russet_core::error::Result;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::daemon::{
    self as p, DaemonLogBatch, DaemonLogBatchParams, DaemonLogRecord, DaemonLogsSubscribeParams,
    DaemonLogsSubscribeResult, DaemonPingParams, DaemonPingResult, DaemonSetLogFilterParams, DaemonSetLogFilterResult,
    DaemonShutdownParams, DaemonShutdownResult, DaemonStatusParams, DaemonStatusResult, ShutdownReason, TraceLevel,
};
use tokio::sync::broadcast::error::RecvError;

use crate::DaemonCtx;
use crate::subs::{BATCH_WINDOW, DropQueue, LOG_QUEUE, wait_for_room};

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::DaemonPing, _, _>(ping);
    r.request::<p::DaemonStatus, _, _>(status);
    r.request::<p::DaemonShutdown, _, _>(shutdown);
    r.request::<p::DaemonSetLogFilter, _, _>(set_log_filter);
    r.request::<p::DaemonLogsSubscribe, _, _>(logs_subscribe);
}

async fn ping(_cx: DaemonCtx, _rq: RequestCtx, _p: DaemonPingParams) -> Result<DaemonPingResult> {
    Ok(DaemonPingResult {})
}

/// The devices the device plane knows, without starting it.
fn device_count(cx: &DaemonCtx) -> u32 {
    let Some(manager) = cx.devices.started() else { return 0 };
    // Until W4 fills it, DeviceManager::devices is an F0 stub.
    crate::daemon::panics::catch_expected(|| manager.devices().len())
        .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
}

/// `daemon.status` as the daemon sees itself.
pub fn status_of(cx: &DaemonCtx) -> DaemonStatusResult {
    DaemonStatusResult {
        version: cx.daemon.version.to_owned(),
        build_id: cx.daemon.build_id.to_owned(),
        pid: cx.daemon.pid,
        uptime_ms: cx.daemon.uptime_ms(),
        endpoint: cx.daemon.endpoint(),
        web: cx.web.url(),
        connections: cx.daemon.connections.list(),
        jobs: cx.jobs.counts(),
        leases: cx.leases.list(),
        devices: device_count(cx),
        rss_bytes: russet_core::platform::rss_bytes().unwrap_or(0),
        started_at: cx.daemon.started_at.clone(),
        device_backend: cx.daemon.device_backend.clone(),
    }
}

async fn status(cx: DaemonCtx, _rq: RequestCtx, _p: DaemonStatusParams) -> Result<DaemonStatusResult> {
    Ok(status_of(&cx))
}

/// Accepts when forced, or when nothing is active (for `upgrade`, also no other connection); the daemon stops
/// right after the reply.
async fn shutdown(cx: DaemonCtx, rq: RequestCtx, p: DaemonShutdownParams) -> Result<DaemonShutdownResult> {
    let reason = p.reason.unwrap_or(ShutdownReason::Request);
    let counts = cx.jobs.counts();
    let active = counts.queued + counts.running + u32::try_from(cx.leases.count()).unwrap_or(u32::MAX);
    let others = cx.daemon.connections.list().iter().filter(|c| c.id != rq.peer.connection_id()).count();
    let accepted = p.force || (active == 0 && (reason != ShutdownReason::Upgrade || others == 0));
    if accepted {
        let daemon = cx.daemon.clone();
        tokio::spawn(async move {
            // Let the reply leave first.
            tokio::time::sleep(Duration::from_millis(50)).await;
            daemon.request_shutdown(reason, p.force);
        });
    }
    Ok(DaemonShutdownResult { accepted, active_sessions: active })
}

async fn set_log_filter(
    cx: DaemonCtx,
    _rq: RequestCtx,
    p: DaemonSetLogFilterParams,
) -> Result<DaemonSetLogFilterResult> {
    let previous = cx.daemon.logs.set_filter(&p.filter)?;
    tracing::info!(filter = %p.filter, "log filter changed");
    Ok(DaemonSetLogFilterResult { previous })
}

async fn logs_subscribe(
    cx: DaemonCtx,
    rq: RequestCtx,
    p: DaemonLogsSubscribeParams,
) -> Result<DaemonLogsSubscribeResult> {
    let min = p.min_level.unwrap_or(TraceLevel::Info);
    let sub = cx.subs.open(&rq.peer, "daemon.logs");
    let mut rx = cx.daemon.logs.subscribe();
    let queue: Arc<DropQueue<DaemonLogRecord>> = Arc::new(DropQueue::new(LOG_QUEUE));
    let (peer, id, cancel) = (rq.peer.clone(), sub.id.clone(), sub.cancel.clone());
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => return,
                r = rx.recv() => match r {
                    Ok(record) if record.level >= min => queue.push((*record).clone()),
                    Ok(_) => continue,
                    Err(RecvError::Lagged(n)) => queue.note_dropped(n),
                    Err(RecvError::Closed) => return,
                },
            }
            // Collect for one batch window, then send while the connection keeps up.
            let window = tokio::time::sleep(BATCH_WINDOW);
            tokio::pin!(window);
            loop {
                tokio::select! {
                    () = &mut window => break,
                    () = cancel.cancelled() => return,
                    r = rx.recv() => match r {
                        Ok(record) if record.level >= min => queue.push((*record).clone()),
                        Ok(_) => {}
                        Err(RecvError::Lagged(n)) => queue.note_dropped(n),
                        Err(RecvError::Closed) => break,
                    },
                }
            }
            if !wait_for_room(&peer, &cancel).await {
                return;
            }
            let (records, dropped) = queue.take(1000);
            if records.is_empty() && dropped == 0 {
                continue;
            }
            let batch = DaemonLogBatchParams {
                subscription_id: id.clone(),
                records,
                dropped: (dropped > 0).then_some(dropped),
            };
            if peer.notify::<DaemonLogBatch>(batch).is_err() {
                return;
            }
        }
    });
    Ok(DaemonLogsSubscribeResult { subscription_id: sub.id })
}
