// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The broker against the fake iOS debugserver (no toolchain needed): pre-attach, the relay rules of RD §6.4,
//! the `QSetDetachOnError` ownership, `D` and `k`, the lost-debugger policy, the handoff timeout and
//! `terminate` without a debugger.

// Unix sockets, /proc and lldb-server: these harnesses run on Linux (and macOS) hosts.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use russet_core::error::codes;
use russet_debug::broker::{Broker, BrokerOptions, BrokerState, EndpointSpec, LostPolicy};
use russet_debug::{BoxStream, BrokerEndpoint, TerminateMode};
use russet_gdbremote::{GdbClient, GdbError, HandshakeOptions, StopEvent};
use russet_testkit::debug::fake_debugserver::LostOutcome;
use russet_testkit::debug::{FakeDebugserver, FakeImage, FakeSpec, ResumeStep, macho};

const PID: u32 = 932;

fn fake(steps: Vec<ResumeStep>) -> FakeDebugserver {
    FakeDebugserver::start(FakeSpec {
        pid: PID,
        image: Some(FakeImage {
            device_path: "/private/var/containers/Bundle/Application/1C2/RussetProbe.app/RussetProbe".into(),
            macho: macho::synthetic_ios_executable([0x42; 16]),
        }),
        steps,
        ..FakeSpec::default()
    })
    .unwrap()
}

async fn upstream(f: &FakeDebugserver) -> BoxStream {
    Box::new(tokio::net::TcpStream::connect(f.addr()).await.unwrap())
}

async fn debugger(endpoint: &BrokerEndpoint) -> GdbClient<BoxStream> {
    let stream: BoxStream = match endpoint {
        BrokerEndpoint::Unix { path } => Box::new(tokio::net::UnixStream::connect(path).await.unwrap()),
        BrokerEndpoint::Tcp { addr } => Box::new(tokio::net::TcpStream::connect(addr).await.unwrap()),
    };
    // LLDB's own handshake, in ack mode first.
    GdbClient::handshake(stream, HandshakeOptions { host_info: true, ..HandshakeOptions::default() }).await.unwrap().0
}

fn opts() -> BrokerOptions {
    BrokerOptions { handoff_timeout: Some(Duration::from_secs(10)), ..BrokerOptions::default() }
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_attach_then_relay_then_detach() {
    let fake = fake(vec![ResumeStep::Output(b"PROBE seq=0\n".to_vec()), ResumeStep::breakpoint(0x1_0000_1004)]);
    let dir = common::scratch("broker-relay");
    let mut b = Broker::connect(upstream(&fake).await, opts()).await.unwrap();
    assert_eq!(b.server().page_size, 16384);
    assert_eq!(b.server().ostype.as_deref(), Some("ios"));
    let stop = b.attach(PID).await.unwrap();
    assert_eq!(stop.signal, 0x11, "vAttach stops the suspended app with SIGSTOP");
    let report = fake.report();
    assert_eq!(report.detach_on_error, Some(0), "Russet-launched apps die with a lost connection");
    assert!(report.position("QSetDetachOnError:0") < report.position("vAttach;3a4"), "set before the attach");

    let handle = b.serve(EndpointSpec::Unix { path: dir.join("sessions/ses_x/gdb.sock") }).await.unwrap();
    assert!(handle.lldb_url().starts_with("unix-connect:///"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let BrokerEndpoint::Unix { path } = handle.endpoint() else { unreachable!() };
        assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
    assert_eq!(handle.state(), BrokerState::Ready);

    let mut lldb = debugger(handle.endpoint()).await;
    assert!(lldb.server().compressions.is_empty(), "SupportedCompressions is stripped");
    assert!(handle.wait_for(|s| s == BrokerState::Relaying).await == BrokerState::Relaying);
    // The process is already stopped: `?` answers at once (what makes a sync `process connect` return).
    match lldb.stop_reason().await.unwrap() {
        StopEvent::Stopped(r) => assert_eq!(r.signal, 0x11),
        other => panic!("{other:?}"),
    }
    // LLDB may not attach or launch; its QSetDetachOnError never reaches debugserver.
    match lldb.attach(PID).await {
        Err(GdbError::Remote { code: 0x16, message: Some(m) }) => {
            assert_eq!(m, "russet broker already attached pid 932")
        }
        other => panic!("{other:?}"),
    }
    lldb.request_ok(b"QSetDetachOnError:1").await.unwrap();
    assert_eq!(fake.report().detach_on_error, Some(0));
    // Breakpoints and resumes go through.
    lldb.set_breakpoint(0x1_0000_1004, 4).await.unwrap();
    lldb.resume(russet_gdbremote::ResumeAction::ContinueAll).await.unwrap();
    assert_eq!(lldb.wait_stop().await.unwrap(), StopEvent::Output(b"PROBE seq=0\n".to_vec()));
    match lldb.wait_stop().await.unwrap() {
        StopEvent::Stopped(r) => assert_eq!(r.reg(32), Some(0x1_0000_1004)),
        other => panic!("{other:?}"),
    }
    // `D` takes the broker's detach path: the app keeps running.
    let mut events = handle.subscribe();
    lldb.detach().await.unwrap();
    assert_eq!(handle.wait_for(|_| false).await, BrokerState::Detached);
    let report = fake.report();
    assert!(report.detached && !report.killed && report.lost.is_none(), "{report:?}");
    assert!(report.breakpoints.contains(&0x1_0000_1004));
    let mut saw_detached = false;
    while let Ok(e) = events.try_recv() {
        saw_detached |= e == russet_debug::broker::BrokerEvent::DebuggerDetached;
    }
    assert!(saw_detached);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_debugger_that_vanishes_gets_the_kill_policy() {
    let fake = fake(vec![]);
    let mut b = Broker::connect(upstream(&fake).await, opts()).await.unwrap();
    b.attach(PID).await.unwrap();
    let handle = b.serve(EndpointSpec::Tcp { bind: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
    assert!(handle.lldb_url().starts_with("connect://127.0.0.1:"));
    let lldb = debugger(handle.endpoint()).await;
    drop(lldb);
    let end = handle.wait_for(|_| false).await;
    assert!(matches!(end, BrokerState::Exited { signal: Some(9), .. }), "{end:?}");
    assert!(fake.report().killed);
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_policy_accepts_the_next_debugger() {
    let fake = fake(vec![ResumeStep::Run]);
    let mut b = Broker::connect(upstream(&fake).await, BrokerOptions { on_debugger_lost: LostPolicy::Keep, ..opts() })
        .await
        .unwrap();
    b.attach(PID).await.unwrap();
    let handle = b.serve(EndpointSpec::Tcp { bind: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
    {
        let mut lldb = debugger(handle.endpoint()).await;
        // Leave it running: the broker must interrupt before handing over again.
        lldb.resume(russet_gdbremote::ResumeAction::ContinueAll).await.unwrap();
    }
    assert_eq!(handle.wait_for(|s| s == BrokerState::Ready).await, BrokerState::Ready);
    let mut lldb = debugger(handle.endpoint()).await;
    match lldb.stop_reason().await.unwrap() {
        StopEvent::Stopped(r) => assert_eq!(r.signal, 0x11, "stopped by the broker's interrupt"),
        other => panic!("{other:?}"),
    }
    lldb.kill().await.unwrap();
    assert!(matches!(handle.wait_for(|_| false).await, BrokerState::Exited { .. }));
    assert_eq!(fake.report().attach_count, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn terminate_without_a_debugger_kills_or_detaches() {
    for (mode, killed) in [(TerminateMode::Kill, true), (TerminateMode::Detach, false)] {
        let fake = fake(vec![]);
        let mut b = Broker::connect(upstream(&fake).await, opts()).await.unwrap();
        b.attach(PID).await.unwrap();
        let handle = b.serve(EndpointSpec::Tcp { bind: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
        handle.terminate(mode).await.unwrap();
        let report = fake.report();
        assert_eq!(report.killed, killed, "{mode:?}: {report:?}");
        assert_eq!(report.detached, !killed, "{mode:?}: {report:?}");
        assert!(handle.state().is_final());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn terminate_while_a_debugger_runs_the_app() {
    let fake = fake(vec![ResumeStep::Run]);
    let mut b = Broker::connect(upstream(&fake).await, opts()).await.unwrap();
    b.attach(PID).await.unwrap();
    let handle = b.serve(EndpointSpec::Tcp { bind: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
    let mut lldb = debugger(handle.endpoint()).await;
    lldb.resume(russet_gdbremote::ResumeAction::ContinueAll).await.unwrap();
    // Once the resume reached debugserver, the app runs and a detach must interrupt it first.
    assert!(fake.wait_for(Duration::from_secs(5), |r| r.saw("vCont;c")));
    handle.terminate(TerminateMode::Detach).await.unwrap();
    assert_eq!(handle.state(), BrokerState::Detached);
    let report = fake.report();
    assert!(
        report.detached && report.transcript.iter().any(|p| p == "\u{3}"),
        "interrupted, then detached: {report:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_handoff_deadline_kills_the_waiting_app() {
    let fake = fake(vec![]);
    let mut b = Broker::connect(
        upstream(&fake).await,
        BrokerOptions { handoff_timeout: Some(Duration::from_millis(200)), ..opts() },
    )
    .await
    .unwrap();
    b.attach(PID).await.unwrap();
    let handle = b.serve(EndpointSpec::Tcp { bind: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
    assert_eq!(handle.wait_for(|_| false).await, BrokerState::HandoffTimedOut);
    assert!(fake.report().killed);
}

#[tokio::test(flavor = "multi_thread")]
async fn attach_failures_have_debug_codes() {
    let fake = fake(vec![]);
    let mut b = Broker::connect(upstream(&fake).await, opts()).await.unwrap();
    let e = b.attach(PID + 1).await.unwrap_err();
    assert_eq!(e.code(), codes::debug::PROCESS_EXITED);
    assert_eq!(e.to_data().details.unwrap()["deviceMessage"], "no such process");
    assert!(e.hint().is_some());
    // A dead upstream is debugserverUnavailable.
    let (a, b2) = tokio::io::duplex(64);
    drop(b2);
    let e = Broker::connect(Box::new(a), opts()).await.unwrap_err();
    assert_eq!(e.code(), codes::debug::DEBUGSERVER_UNAVAILABLE);
    let _ = LostOutcome::Killed;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_broker_removes_a_departed_debuggers_breakpoints_before_it_detaches() {
    for policy in [LostPolicy::Detach, LostPolicy::Keep] {
        let fake = fake(vec![ResumeStep::Run]);
        let mut b =
            Broker::connect(upstream(&fake).await, BrokerOptions { on_debugger_lost: policy, ..opts() }).await.unwrap();
        b.attach(PID).await.unwrap();
        let handle = b.serve(EndpointSpec::Tcp { bind: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
        {
            let mut lldb = debugger(handle.endpoint()).await;
            lldb.set_breakpoint(0x1_0000_1004, 4).await.unwrap();
            lldb.request_ok(b"Z2,100008000,8").await.unwrap();
            lldb.resume(russet_gdbremote::ResumeAction::ContinueAll).await.unwrap();
            assert!(fake.wait_for(Duration::from_secs(5), |r| r.saw("vCont;c")));
        }
        let state = handle.wait_for(|s| s == BrokerState::Ready).await;
        let report = fake.report();
        assert!(report.breakpoints.is_empty(), "{policy:?}: {report:?}");
        assert!(report.saw("z2,100008000,8"), "{policy:?}: watchpoints too");
        match policy {
            LostPolicy::Detach => assert!(report.detached && state == BrokerState::Detached, "{report:?}"),
            _ => assert_eq!(state, BrokerState::Ready),
        }
    }
}
