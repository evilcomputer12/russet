// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Swift Quick Relaunch over a scripted device (RD §4.7, W11): a `run` session relaunches in place — same
//! session id, a new pid, still running, and the old process is signalled. A stale exit watcher of the replaced
//! process does not end the relaunched session (the launch-generation guard).

#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use russet_core::Result;
use russet_debug::launch::{LaunchSpec, Launched, ProcessExit};
use russet_debug::{
    DebugConfig, DebugService, DeviceServices, RunRequest, SessionLease, WireSessionState, async_trait,
};
use russet_device::{DeviceSelector, Udid};

/// A device that hands out a fresh pid on every launch, whose process never exits on its own, and that records
/// the signals it is sent.
struct RelaunchDevice {
    next_pid: AtomicU32,
    signals: Mutex<Vec<(u32, i32)>>,
    launches: AtomicU32,
}

#[async_trait]
impl DeviceServices for RelaunchDevice {
    async fn resolve(&self, _sel: &DeviceSelector) -> Result<Udid> {
        Ok(Udid("u".into()))
    }

    async fn launch(&self, _udid: &Udid, _spec: &LaunchSpec) -> Result<Launched> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let pid = self.next_pid.fetch_add(1, Ordering::SeqCst);
        let mut l = Launched::pid_only(pid);
        l.executable = Some("/Probe.app/Probe".into());
        // The process stays up until it is signalled: a future that never resolves.
        l.exit = Some(Box::pin(std::future::pending::<ProcessExit>()));
        Ok(l)
    }

    async fn signal(&self, _udid: &Udid, pid: u32, sig: i32) -> Result<()> {
        self.signals.lock().unwrap().push((pid, sig));
        Ok(())
    }
}

#[tokio::test]
async fn quick_relaunch_keeps_the_session_and_swaps_the_process() {
    let dev = std::sync::Arc::new(RelaunchDevice {
        next_pid: AtomicU32::new(100),
        signals: Mutex::new(Vec::new()),
        launches: AtomicU32::new(0),
    });
    let svc = DebugService::new(dev.clone(), DebugConfig::default());
    let mut req = RunRequest::new(DeviceSelector::Only, "dev.russet.test.probe");
    req.lease = SessionLease::Detached;
    let h = svc.run(req).await.unwrap();
    let sid = h.id().clone();
    assert_eq!(h.pid(), Some(100));
    assert_eq!(h.summary().state, WireSessionState::Running);

    // Quick Relaunch: new pid, same session, still running.
    let (pid, _ms) = svc.relaunch(sid.as_str()).await.unwrap();
    assert_eq!(pid, 101);
    assert_eq!(h.pid(), Some(101));
    assert_eq!(h.id(), &sid);
    assert_eq!(dev.launches.load(Ordering::SeqCst), 2);
    // The old process (100) was signalled (killed) by the swap.
    assert!(dev.signals.lock().unwrap().iter().any(|(p, _)| *p == 100));

    // The relaunched session survives: the stale exit watcher of pid 100 must not have ended it.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!h.is_ended(), "the relaunched session was ended by a stale watcher");
    assert_eq!(h.summary().state, WireSessionState::Running);

    // A second relaunch keeps advancing the pid.
    let (pid, _ms) = svc.relaunch(sid.as_str()).await.unwrap();
    assert_eq!(pid, 102);
    assert_eq!(h.pid(), Some(102));

    // run.list reports the one session (not three).
    assert_eq!(svc.sessions().iter().filter(|s| s.session_id == sid).count(), 1);
}

#[tokio::test]
async fn relaunch_of_an_unknown_session_is_an_error() {
    let dev = std::sync::Arc::new(RelaunchDevice {
        next_pid: AtomicU32::new(1),
        signals: Mutex::new(Vec::new()),
        launches: AtomicU32::new(0),
    });
    let svc = DebugService::new(dev, DebugConfig::default());
    let e = svc.relaunch("ses_does_not_exist").await.unwrap_err();
    assert_eq!(e.code(), russet_core::error::codes::debug::SESSION_NOT_FOUND);
}
