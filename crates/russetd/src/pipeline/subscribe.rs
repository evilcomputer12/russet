// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `run.subscribe`: turns session events and output into `run.state`, `run.output` and `run.exited`
//! notifications (ARCHITECTURE §9.1).
//!
//! Output is batched for up to 50 ms (at most 200 entries per notification); a subscriber that falls behind
//! loses the oldest entries and the next `run.output` carries `dropped` (§6.4). The forwarder is written over a
//! `send` callback, so it is tested without a connection; the handler sends through the requester's peer.

use std::future::Future;
use std::time::Duration;

use russet_core::ids::SubscriptionId;
use russet_debug::{SessionEvent, SessionHandle};
use russet_protocol::run::{RunExitedParams, RunOutputParams, RunStateParams};
use tokio::sync::broadcast::error::RecvError;

/// The batching window of `run.output`.
pub const OUTPUT_WINDOW: Duration = Duration::from_millis(50);
/// The largest `run.output` batch.
pub const OUTPUT_BATCH: usize = 200;

/// One notification for the subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunNotification {
    State(RunStateParams),
    Output(RunOutputParams),
    Exited(RunExitedParams),
}

/// Forwards one session until it ends, `send` returns `false` (the subscriber is gone), or `stop` resolves.
/// It first sends the current state, so a late subscriber knows where the session is.
pub async fn forward_session<S, F>(h: SessionHandle, sub: SubscriptionId, send: S, stop: F)
where
    S: Fn(RunNotification) -> bool,
    F: Future<Output = ()>,
{
    let mut events = h.subscribe();
    let mut output = h.logs().subscribe();
    let sid = h.id().clone();
    let state = |s: russet_protocol::common::SessionState, detail: Option<String>| {
        RunNotification::State(RunStateParams {
            subscription_id: sub.clone(),
            session_id: sid.clone(),
            state: s,
            detail,
        })
    };
    let exited = |h: &SessionHandle| {
        let summary = h.summary();
        let exit = summary.exit;
        RunNotification::Exited(RunExitedParams {
            subscription_id: sub.clone(),
            session_id: sid.clone(),
            state: Some(summary.state),
            code: exit.as_ref().and_then(|e| e.code),
            signal: exit.as_ref().and_then(|e| e.signal),
            crash_id: exit.and_then(|e| e.crash_id),
            error: h.error(),
        })
    };
    if !send(state(h.summary().state, None)) {
        return;
    }
    if h.is_ended() {
        let _ = send(exited(&h));
        return;
    }
    tokio::pin!(stop);
    let mut batch = Vec::new();
    let mut dropped = 0u64;
    let mut tick = tokio::time::interval(OUTPUT_WINDOW);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let flush = |batch: &mut Vec<russet_protocol::common::LogEntry>, dropped: &mut u64| -> bool {
        if batch.is_empty() && *dropped == 0 {
            return true;
        }
        let n = RunNotification::Output(RunOutputParams {
            subscription_id: sub.clone(),
            session_id: sid.clone(),
            entries: std::mem::take(batch),
            dropped: (*dropped > 0).then_some(*dropped),
        });
        *dropped = 0;
        send(n)
    };
    loop {
        tokio::select! {
            () = &mut stop => break,
            e = events.recv() => match e {
                Ok(SessionEvent::State(s)) => {
                    if !flush(&mut batch, &mut dropped) || !send(state(s.wire(), None)) {
                        break;
                    }
                }
                Ok(SessionEvent::Debugger(d)) => {
                    let detail = format!("debugger {}", match d {
                        russet_debug::DebuggerState::None => "none",
                        russet_debug::DebuggerState::Attached => "attached",
                        russet_debug::DebuggerState::Detached => "detached",
                    });
                    if !send(state(h.summary().state, Some(detail))) {
                        break;
                    }
                }
                Ok(SessionEvent::Ended { .. }) | Err(RecvError::Closed) => {
                    // Drain what the output channel still holds, then report the end.
                    while let Ok(entry) = output.try_recv() {
                        batch.push(entry);
                    }
                    let _ = flush(&mut batch, &mut dropped);
                    let _ = send(exited(&h));
                    break;
                }
                Err(RecvError::Lagged(_)) => {}
            },
            o = output.recv() => match o {
                Ok(entry) => {
                    batch.push(entry);
                    if batch.len() >= OUTPUT_BATCH && !flush(&mut batch, &mut dropped) {
                        break;
                    }
                }
                Err(RecvError::Lagged(n)) => dropped += n,
                Err(RecvError::Closed) => {}
            },
            _ = tick.tick() => {
                if !flush(&mut batch, &mut dropped) {
                    break;
                }
            }
        }
    }
}
