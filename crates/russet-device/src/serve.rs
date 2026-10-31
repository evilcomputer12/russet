// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The connection-bound halves of the device methods (ARCHITECTURE §6.3 leases, §6.4 subscriptions, §9.1
//! notifications): what russetd's handlers run after they answered. They take a notifier closure (`false`: the
//! connection is gone) and the futures that say when the connection closed, so they are tested here without a
//! daemon; the handlers only adapt `Peer::notify` and `Peer::closed`.
//!
//! - **Leases** (`forward`, `app-launch`, `device-reservation`) end when their owner is gone: the connection closed
//!   and `[device] lease-grace` (2 s) passed ([`owner_gone`]).
//! - **Subscriptions** (`device.watch` → `device.changed`, `logs.subscribe` → `logs.entries`) end with the
//!   connection.
//! - **`app.launch`** sends the console as `app.output` (lines from before the task started first) and `app.exited`
//!   when the app ends. Ending the lease drops the launch channel; the app keeps running (DC-04 D2).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use russet_core::ids::{LeaseId, SubscriptionId};
use russet_protocol::{app as pa, device as pd, logs as pl};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::debug::ForwardHandle;
use crate::logs::LogStream;
use crate::manager::{DeviceEvent, DeviceManager};
use crate::process::LaunchedProcess;

/// Resolves once `closed` (the owner's connection ended) resolved and `grace` passed (ARCHITECTURE §6.3).
pub async fn owner_gone(closed: impl Future<Output = ()>, grace: Duration) {
    closed.await;
    tokio::time::sleep(grace).await;
}

/// Holds a `forward` lease: the forward stays open until it is closed (`forward.close`, its accept policy, a
/// detach) or its owner is gone.
pub async fn hold_forward(handle: ForwardHandle, owner_gone: impl Future<Output = ()>) {
    tokio::select! {
        _ = owner_gone => {}
        _ = handle.closed() => {}
    }
    drop(handle);
}

/// Holds a `device-reservation` lease: released when its owner is gone (`lease.release` and a detach end it
/// earlier).
pub async fn hold_reservation(m: Arc<DeviceManager>, lease: LeaseId, owner_gone: impl Future<Output = ()>) {
    owner_gone.await;
    m.release(&lease);
}

/// `device.watch`: every registry event as `device.changed` until the connection closes or `notify` fails.
pub async fn serve_watch(
    subscription_id: SubscriptionId,
    mut events: broadcast::Receiver<DeviceEvent>,
    closed: impl Future<Output = ()>,
    mut notify: impl FnMut(pd::DeviceChangedParams) -> bool,
) {
    tokio::pin!(closed);
    loop {
        let ev = tokio::select! {
            _ = &mut closed => return,
            ev = events.recv() => ev,
        };
        match ev {
            Ok(ev) => {
                let (change, device) = crate::rpc::device_change(&ev);
                if !notify(pd::DeviceChangedParams { subscription_id: subscription_id.clone(), change, device }) {
                    return;
                }
            }
            // A slow watcher missed events; the next one carries the device's current summary.
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        }
    }
}

/// `logs.subscribe`: 50 ms batches as `logs.entries` (`dropped` after a slow consumer, ARCHITECTURE §6.4) until the
/// sources end, the connection closes or `notify` fails.
pub async fn serve_logs(
    subscription_id: SubscriptionId,
    mut stream: LogStream,
    closed: impl Future<Output = ()>,
    mut notify: impl FnMut(pl::LogsEntriesParams) -> bool,
) {
    tokio::pin!(closed);
    loop {
        let batch = tokio::select! {
            _ = &mut closed => break,
            b = stream.next_batch() => b,
        };
        let Some(batch) = batch else { break };
        let params = pl::LogsEntriesParams {
            subscription_id: subscription_id.clone(),
            entries: batch.entries,
            dropped: (batch.dropped > 0).then_some(batch.dropped),
        };
        if !notify(params) {
            break;
        }
    }
    stream.close();
}

/// What an `app-launch` lease sends to its owner.
#[derive(Debug, Clone, PartialEq)]
pub enum LaunchEvent {
    Output(pa::AppOutputParams),
    Exited(pa::AppExitedParams),
}

/// Serves an `app-launch` lease: console lines as `app.output`, then `app.exited`, until the app exits, the owner
/// is gone, or `notify` fails. The launch channel (inside `launched`) closes when this returns.
pub async fn serve_launch(
    lease_id: LeaseId,
    launched: LaunchedProcess,
    owner_gone: impl Future<Output = ()>,
    mut notify: impl FnMut(LaunchEvent) -> bool,
) {
    let mut exit = launched.exit.clone();
    let mut output = launched.output.as_ref().map(|o| o.subscribe());
    tokio::pin!(owner_gone);
    let line = |lease_id: &LeaseId, data: String| {
        LaunchEvent::Output(pa::AppOutputParams {
            lease_id: lease_id.clone(),
            stream: pa::AppOutputStream::Stdout,
            data,
        })
    };
    // Lines written between the launch and this task first.
    if let Some((earlier, _)) = output.as_mut() {
        for e in std::mem::take(earlier) {
            if !notify(line(&lease_id, e.message)) {
                return;
            }
        }
    }
    loop {
        // `Some(Some(line))`, `Some(None)` after a lag (lines were lost; keep going), `None` at the console's end.
        let next_line = async {
            match output.as_mut() {
                Some((_, rx)) => match rx.recv().await {
                    Ok(e) => Some(Some(e)),
                    Err(RecvError::Lagged(_)) => Some(None),
                    Err(RecvError::Closed) => None,
                },
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = &mut owner_gone => return,
            e = exit.wait() => {
                // Lines the app wrote before it ended come first.
                if let Some((_, rx)) = output.as_mut() {
                    while let Ok(entry) = rx.try_recv() {
                        if !notify(line(&lease_id, entry.message)) {
                            return;
                        }
                    }
                }
                let _ = notify(LaunchEvent::Exited(pa::AppExitedParams {
                    lease_id: lease_id.clone(),
                    pid: launched.pid,
                    code: e.code,
                    signal: e.signal,
                    crash_id: None,
                }));
                return;
            }
            l = next_line => match l {
                Some(None) => {}
                Some(Some(entry)) => {
                    if !notify(line(&lease_id, entry.message)) {
                        return;
                    }
                }
                // The console ended: keep waiting for the exit only.
                None => output = None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use russet_protocol::common::{LogEntry, LogLevel, LogSource};
    use time::OffsetDateTime;

    use super::*;
    use crate::logs::Batcher;
    use crate::process::{ExitInfo, LaunchVia, OutputHub, exit_channel};

    fn entry(msg: &str) -> LogEntry {
        LogEntry {
            ts: "2026-10-11T09:00:00.000Z".into(),
            source: LogSource::Oslog,
            level: LogLevel::Default,
            message: msg.into(),
            pid: Some(932),
            tid: None,
            process: Some("RussetProbe".into()),
            subsystem: None,
            category: None,
            image_uuid: None,
            image_offset: None,
            mach_ts: None,
        }
    }

    fn data(ev: &LaunchEvent) -> String {
        match ev {
            LaunchEvent::Output(o) => o.data.clone(),
            LaunchEvent::Exited(x) => format!("exited pid {} signal {:?}", x.pid, x.signal),
        }
    }

    #[tokio::test]
    async fn a_launch_lease_sends_its_console_then_the_exit() {
        let hub = Arc::new(OutputHub::new(932, Some("RussetProbe".into())));
        hub.push_line("written before the lease task started");
        let (reporter, watcher) = exit_channel();
        let launched = LaunchedProcess::from_parts(
            932,
            "/x/RussetProbe".into(),
            LaunchVia::CoreDevice,
            Some(Arc::clone(&hub)),
            watcher,
        );
        let lease = crate::ids::new_lease_id();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = Arc::clone(&seen);
        let (_owner, owner_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(serve_launch(
            lease.clone(),
            launched,
            async {
                let _ = owner_rx.await;
            },
            move |e| {
                s2.lock().expect("lock").push(e);
                true
            },
        ));
        for _ in 0..100 {
            if !seen.lock().expect("lock").is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        hub.push_line("live line");
        hub.push_line("last words");
        // The exit is reported right after the last lines: they still come first.
        reporter.report(ExitInfo {
            code: None,
            signal: Some(9),
            at: OffsetDateTime::now_utc(),
            detected_by: "console",
        });
        tokio::time::timeout(Duration::from_secs(5), task).await.expect("the lease ends with the app").expect("join");
        let seen = seen.lock().expect("lock");
        assert_eq!(
            seen.iter().map(data).collect::<Vec<_>>(),
            ["written before the lease task started", "live line", "last words", "exited pid 932 signal Some(9)"]
        );
        assert!(seen.iter().all(|e| match e {
            LaunchEvent::Output(o) => o.lease_id == lease && o.stream == pa::AppOutputStream::Stdout,
            LaunchEvent::Exited(x) => x.lease_id == lease,
        }));
    }

    #[tokio::test(start_paused = true)]
    async fn a_launch_lease_ends_when_its_owner_is_gone_and_leaves_the_app_alone() {
        let (reporter, watcher) = exit_channel();
        let launched = LaunchedProcess::from_parts(933, String::new(), LaunchVia::Dvt, None, watcher);
        let exit = launched.exit.clone();
        let (owner, owner_rx) = tokio::sync::oneshot::channel::<()>();
        let sent = Arc::new(Mutex::new(0usize));
        let s2 = Arc::clone(&sent);
        let gone = owner_gone(
            async {
                let _ = owner_rx.await;
            },
            Duration::from_secs(2),
        );
        let task = tokio::spawn(serve_launch(crate::ids::new_lease_id(), launched, gone, move |_| {
            *s2.lock().expect("lock") += 1;
            true
        }));
        drop(owner);
        tokio::time::sleep(Duration::from_millis(1900)).await;
        assert!(!task.is_finished(), "within the grace");
        tokio::time::timeout(Duration::from_millis(200), task).await.expect("ended after the grace").expect("join");
        assert_eq!(*sent.lock().expect("lock"), 0, "no app.exited: the app was not touched");
        assert!(exit.exited().is_none());
        drop(reporter);
    }

    #[tokio::test(start_paused = true)]
    async fn log_subscriptions_send_batches_with_drops_until_the_connection_closes() {
        let batcher = Arc::new(Batcher::new(3));
        let stream = LogStream::from_batcher(Arc::clone(&batcher));
        for i in 0..5 {
            batcher.push(entry(&format!("m{i}")));
        }
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel::<()>();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = Arc::clone(&seen);
        let sub = crate::ids::new_subscription_id();
        let task = tokio::spawn(serve_logs(
            sub.clone(),
            stream,
            async {
                let _ = closed_rx.await;
            },
            move |n| {
                s2.lock().expect("lock").push(n);
                true
            },
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        {
            let seen = seen.lock().expect("lock");
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].subscription_id, sub);
            assert_eq!(seen[0].entries.iter().map(|e| e.message.as_str()).collect::<Vec<_>>(), ["m2", "m3", "m4"]);
            assert_eq!(seen[0].dropped, Some(2));
        }
        drop(closed_tx);
        tokio::time::timeout(Duration::from_secs(1), task).await.expect("ends with the connection").expect("join");
        // A failing notify (the connection is gone) ends it too.
        let batcher = Arc::new(Batcher::new(10));
        batcher.push(entry("x"));
        let (_keep, never) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(serve_logs(
            crate::ids::new_subscription_id(),
            LogStream::from_batcher(batcher),
            async {
                let _ = never.await;
            },
            |_| false,
        ));
        tokio::time::timeout(Duration::from_secs(1), task).await.expect("ends when notify fails").expect("join");
    }
}
