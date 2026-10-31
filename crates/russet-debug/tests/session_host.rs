// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Sessions end to end on the host (PLAN §4.7 tests): `DebugService` launches the host probe suspended, the
//! broker attaches through lldb-server, and LLDB runs the generated batch script exactly as `russet debug --batch`
//! does. With `--detach` the probe keeps running and the session stays `running` with `debugger: detached`;
//! without it the probe is killed and the session ends `exited`. Also: `run` without a debugger, the merged
//! output, owned leases ending with their owner, and the `legacy-async` fallback.

// Unix sockets, /proc and lldb-server: these harnesses run on Linux (and macOS) hosts.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::HostDevice;
use russet_debug::lldb::{self, BatchCut, BatchEnd, BatchLimits, BatchSpec, Breakpoint, InitSettings, LldbPlatform};
use russet_debug::logs::OutputFilter;
use russet_debug::{
    AttachMode, DebugConfig, DebugRequest, DebugService, DebugTarget, DebuggerState, Frontend, RunRequest,
    SessionLease, SessionState, TerminateMode, WireSessionState,
};
use russet_device::DeviceSelector;
use russet_testkit::debug::programs::{self, PROBE_C, PROBE_MARKER, build_probe, line_after_marker, process_alive};
use russet_testkit::debug::{LldbTools, ScratchDir};
use russet_toolchain::{Component, Provenance, ToolInfo};

struct Fixture {
    tools: LldbTools,
    dir: ScratchDir,
    dev: Arc<HostDevice>,
    svc: DebugService,
    line: u32,
}

fn fixture(test: &str) -> Option<Fixture> {
    let tools = LldbTools::for_test(test, true)?;
    if programs::find_cc().is_none() {
        eprintln!("skipping {test}: no C compiler");
        return None;
    }
    let dir = common::scratch("session");
    let probe = build_probe(dir.path()).unwrap();
    let dev = HostDevice::new(tools.clone(), probe.path.clone());
    let cfg = DebugConfig {
        runtime_dir: dir.join("run"),
        state_dir: Some(dir.join("state")),
        broker: russet_debug::broker::BrokerOptions {
            handoff_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        },
        ..DebugConfig::default()
    };
    let svc = DebugService::new(dev.clone(), cfg);
    let line = line_after_marker(PROBE_C, PROBE_MARKER).unwrap();
    Some(Fixture { tools, dir, dev, svc, line })
}

fn lldb_tool(tools: &LldbTools) -> ToolInfo {
    ToolInfo {
        component: Component::Lldb,
        version: "21.0.0".into(),
        path: tools.lldb.clone(),
        provenance: Provenance::System,
        env: tools.env.iter().cloned().collect(),
        ok: true,
        problems: Vec::new(),
    }
}

fn debug_request(f: &Fixture, lease: SessionLease, attach_mode: AttachMode) -> DebugRequest {
    let mut run = RunRequest::new(DeviceSelector::Only, "dev.russet.test.probe");
    run.lease = lease;
    run.project = Some(f.dir.path().to_path_buf());
    run.process_name = Some("probe".into());
    DebugRequest {
        run,
        stop_on_entry: false,
        frontend: Frontend::LldbCli,
        attach_mode,
        flavor: russet_protocol::debug::DebuggerFlavor::Swift,
        lldb: Some(lldb_tool(&f.tools)),
        lldb_dap: None,
        target: DebugTarget {
            local_executable: f.dev.program.clone(),
            local_app_dir: None,
            sdk_path: None,
            arch: None,
        },
        platform: LldbPlatform::Host,
        init: InitSettings::default(),
        script_root: None,
    }
}

/// What `russet debug --batch` does after `debug.prepare`: append breakpoints, continue, probe, commands, end,
/// through the CLI's runner (`lldb::run_batch`), with `on_cut` standing in for its `run.stop`.
async fn run_script<I, E, F>(
    f: &Fixture,
    prepared: &russet_debug::PreparedDebug,
    bps: &[Breakpoint],
    end: BatchEnd,
    limits: BatchLimits,
    interrupt: I,
    on_cut: E,
) -> (lldb::BatchTranscript, lldb::BatchRun)
where
    I: std::future::Future<Output = ()>,
    E: FnOnce(BatchCut) -> F,
    F: std::future::Future<Output = ()>,
{
    let commands = vec!["frame variable probeValue".to_owned()];
    let script = lldb::session_script(
        &prepared.lldb.init_commands,
        &prepared.lldb.attach_commands,
        bps,
        false,
        Some(&BatchSpec { commands: commands.clone(), end }),
    );
    if prepared.attach_mode == AttachMode::Broker {
        assert!(!lldb::uses_python(&script), "{script:#?}");
    }
    let path = f.dir.join(format!("batch-{}.lldb", prepared.session.id()));
    std::fs::write(&path, lldb::script_text(&script)).unwrap();
    let mut cmd = tokio::process::Command::new(&f.tools.lldb);
    cmd.args(lldb::batch_args(&path));
    for (k, v) in &f.tools.env {
        cmd.env(k, v);
    }
    let run = lldb::run_batch(cmd, limits, interrupt, on_cut).await.unwrap();
    if std::env::var_os("RUSSET_TEST_SHOW_LLDB").is_some() {
        eprintln!("{}\n{}", run.stdout, run.stderr);
    }
    (lldb::parse_batch_output(&run.stdout, &commands), run)
}

/// A batch to the breakpoint on the probe's marker line, with a 90 s limit.
async fn run_batch(f: &Fixture, prepared: &russet_debug::PreparedDebug, end: BatchEnd) -> lldb::BatchTranscript {
    let bps = vec![Breakpoint { file: "probe.c".into(), line: f.line }];
    let limits = BatchLimits { timeout: Some(Duration::from_secs(90)), ..BatchLimits::default() };
    let (t, run) = run_script(f, prepared, &bps, end, limits, std::future::pending(), |_| async {}).await;
    assert_eq!((run.code, run.cut), (Some(0), None), "{}\n{}", run.stdout, run.stderr);
    assert!(lldb::batch_failure(&t, &run).is_none(), "{}\n{}", run.stdout, run.stderr);
    t
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_detach_leaves_the_app_running_with_a_detached_debugger() {
    let Some(f) = fixture("session --detach") else { return };
    let prepared = f.svc.prepare_debug(debug_request(&f, SessionLease::Detached, AttachMode::Broker)).await.unwrap();
    let s = prepared.session.clone();
    assert_eq!(s.summary().state, WireSessionState::Ready);
    assert_eq!(s.debugger(), DebuggerState::None);
    let script = std::fs::read_to_string(prepared.attach_script.as_ref().unwrap()).unwrap();
    assert!(script.contains("process connect -p gdb-remote \"unix-connect:///"), "{script}");
    assert!(!lldb::uses_python(&script.lines().map(str::to_owned).collect::<Vec<_>>()));

    let t = run_batch(&f, &prepared, BatchEnd::Detach).await;
    let stop = t.stop.clone().expect("a stop");
    assert_eq!(stop.reason, "breakpoint");
    assert_eq!(stop.frame.line, Some(f.line));
    assert_eq!(stop.frame.function.as_deref(), Some("probe_step"));
    assert!(t.commands[0].output.contains("= 42"), "{:?}", t.commands);
    assert!(t.detached && !t.exited);

    // The broker forwarded LLDB's `D`: the app runs on, the session stays `running`, the debugger `detached`.
    let state = s
        .wait_for(Duration::from_secs(10), |st| *st == SessionState::Running && s.debugger() == DebuggerState::Detached)
        .await;
    assert_eq!(state, SessionState::Running);
    let summary = s.summary();
    assert_eq!(
        (summary.state, summary.debugger, summary.lease),
        (WireSessionState::Running, DebuggerState::Detached, SessionLease::Detached)
    );
    let pid = prepared.pid;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(process_alive(pid), "the app keeps running after the detach");
    // Its stdout keeps arriving in the session's output.
    let probe_lines = |svc: &DebugService| {
        let h = svc.session(s.id()).unwrap();
        let only = OutputFilter { contains: Some("PROBE seq=".into()), ..OutputFilter::default() };
        h.logs().query(None, None, &only).0.len()
    };
    let before = probe_lines(&f.svc);
    assert!(
        common::eventually(Duration::from_secs(5), || probe_lines(&f.svc) > before + 3).await,
        "heartbeats keep coming"
    );
    assert_eq!(f.svc.sessions().iter().filter(|x| x.state == WireSessionState::Running).count(), 1);

    // `russet stop`: run.stop {mode: kill}.
    s.terminate(TerminateMode::Kill).await.unwrap();
    assert!(common::eventually(Duration::from_secs(5), || !process_alive(pid)).await);
    let summary = s.summary();
    assert_eq!(summary.state, WireSessionState::Exited);
    assert_eq!(summary.exit.and_then(|e| e.signal), Some(9));
    assert!(summary.ended_at.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_without_detach_kills_the_app_and_ends_exited() {
    let Some(f) = fixture("session kill") else { return };
    let prepared = f.svc.prepare_debug(debug_request(&f, SessionLease::Owned, AttachMode::Broker)).await.unwrap();
    let s = prepared.session.clone();
    let t = run_batch(&f, &prepared, BatchEnd::Kill).await;
    assert_eq!(t.stop.as_ref().map(|s| s.reason.as_str()), Some("breakpoint"));
    assert!(t.exited && !t.detached);
    let end = s.wait_for(Duration::from_secs(10), SessionState::is_final).await;
    assert!(matches!(end, SessionState::Exited { .. }), "{end:?}");
    let summary = s.summary();
    assert_eq!(summary.state, WireSessionState::Exited, "LLDB's kill is not a crash");
    assert_eq!(summary.debugger, DebuggerState::Attached);
    assert!(!process_alive(prepared.pid));
    // session.json and sessions.json follow.
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(f.dir.join("state/sessions").join(s.id().as_str()).join("session.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["state"], "exited");
}

/// A breakpoint that never resolves (W9's failure mode): the time limit ends the session under LLDB, LLDB's
/// blocked `process continue` returns, and the run fails with `debug.batchTimeout`; the app is killed.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_that_never_stops_times_out_and_kills_the_app() {
    let Some(f) = fixture("batch timeout") else { return };
    let prepared = f.svc.prepare_debug(debug_request(&f, SessionLease::Owned, AttachMode::Broker)).await.unwrap();
    let s = prepared.session.clone();
    let never = vec![Breakpoint { file: "nosuch.c".into(), line: 1 }];
    let limits = BatchLimits { timeout: Some(Duration::from_secs(3)), grace: Duration::from_secs(20) };
    let started = std::time::Instant::now();
    let session = s.clone();
    let (t, run) =
        run_script(&f, &prepared, &never, BatchEnd::Kill, limits, std::future::pending(), |cut| async move {
            assert!(matches!(cut, BatchCut::Timeout(_)));
            session.terminate(TerminateMode::Kill).await.unwrap();
        })
        .await;
    assert_eq!(run.cut, Some(BatchCut::Timeout(Duration::from_secs(3))));
    assert!(!run.killed, "LLDB exits by itself once the session ended: {}\n{}", run.stdout, run.stderr);
    assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
    assert!(t.stop.is_none());
    let e = lldb::batch_failure(&t, &run).expect("a failure");
    assert_eq!(e.code(), russet_core::error::codes::debug::BATCH_TIMEOUT);
    assert_eq!(e.code().exit_code(), 9);
    assert!(common::eventually(Duration::from_secs(5), || !process_alive(prepared.pid)).await);
    assert_eq!(s.summary().state, WireSessionState::Exited);
}

/// Ctrl-C with `--detach`: the session is ended with `run.stop {mode: detach}` under LLDB; LLDB exits, the app
/// keeps running untracked, and the run reports `core.cancelled`.
#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_detached_batch_leaves_the_app_running() {
    let Some(f) = fixture("batch interrupt") else { return };
    let prepared = f.svc.prepare_debug(debug_request(&f, SessionLease::Detached, AttachMode::Broker)).await.unwrap();
    let s = prepared.session.clone();
    let never = vec![Breakpoint { file: "nosuch.c".into(), line: 1 }];
    let limits = BatchLimits { timeout: None, grace: Duration::from_secs(20) };
    let interrupt = tokio::time::sleep(Duration::from_secs(2));
    let session = s.clone();
    let (t, run) = run_script(&f, &prepared, &never, BatchEnd::Detach, limits, interrupt, |cut| async move {
        assert_eq!(cut, BatchCut::Interrupted);
        session.terminate(TerminateMode::Detach).await.unwrap();
    })
    .await;
    assert_eq!((run.cut, run.killed), (Some(BatchCut::Interrupted), false), "{}\n{}", run.stdout, run.stderr);
    let e = lldb::batch_failure(&t, &run).expect("a failure");
    assert_eq!(e.code(), russet_core::error::codes::core::CANCELLED);
    // The broker removed LLDB's (pending) breakpoint and detached: the app runs on, its heartbeat continues.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(process_alive(prepared.pid), "the app keeps running after a detach");
    assert!(s.is_ended());
    let _ = std::process::Command::new("kill").args(["-9", &prepared.pid.to_string()]).status();
    assert!(common::eventually(Duration::from_secs(5), || !process_alive(prepared.pid)).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn run_without_a_debugger_streams_output_and_stops() {
    let Some(f) = fixture("run") else { return };
    let mut req = RunRequest::new(DeviceSelector::Only, "dev.russet.test.probe");
    req.process_name = Some("probe".into());
    req.lease = SessionLease::Detached;
    let s = f.svc.run(req).await.unwrap();
    let pid = s.pid().unwrap();
    assert_eq!(s.summary().state, WireSessionState::Running);
    assert_eq!(s.summary().kind, russet_debug::SessionKind::Run);
    let heartbeat = OutputFilter { contains: Some("PROBE seq=".into()), ..OutputFilter::default() };
    assert!(common::eventually(Duration::from_secs(5), || s.logs().query(None, None, &heartbeat).0.len() >= 5).await);
    let (entries, dropped) = s.logs().query(None, Some(2), &heartbeat);
    assert_eq!((entries.len(), dropped), (2, 0));
    assert_eq!(entries[0].pid, Some(pid));
    assert_eq!(entries[0].process.as_deref(), Some("probe"));
    // The default environment reached the app (RD §5.3).
    let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
    assert!(env.split(|b| *b == 0).any(|kv| kv == b"OS_ACTIVITY_DT_MODE=YES"));
    s.terminate(TerminateMode::Kill).await.unwrap();
    assert!(common::eventually(Duration::from_secs(5), || !process_alive(pid)).await);
    assert_eq!(s.summary().state, WireSessionState::Exited);
    assert!(f.dev.signals.lock().unwrap().contains(&(pid, 9)));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owned_session_ends_shortly_after_its_owner() {
    let Some(f) = fixture("owned lease") else { return };
    let mut req = RunRequest::new(DeviceSelector::Only, "dev.russet.test.probe");
    req.lease = SessionLease::Owned;
    let s = f.svc.run(req).await.unwrap();
    let pid = s.pid().unwrap();
    let (gone_tx, gone_rx) = tokio::sync::oneshot::channel::<()>();
    s.end_with_owner(
        async move {
            let _ = gone_rx.await;
        },
        Duration::from_millis(300),
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(process_alive(pid), "nothing happens while the owner is there");
    gone_tx.send(()).unwrap();
    assert!(common::eventually(Duration::from_secs(5), || s.is_ended()).await);
    assert!(common::eventually(Duration::from_secs(5), || !process_alive(pid)).await);
    // A detached session ignores its owner.
    let mut req = RunRequest::new(DeviceSelector::Only, "dev.russet.test.probe");
    req.lease = SessionLease::Detached;
    let d = f.svc.run(req).await.unwrap();
    d.end_with_owner(async {}, Duration::from_millis(10));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!d.is_ended());
    d.terminate(TerminateMode::Kill).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_async_lets_lldb_attach_over_a_plain_forward() {
    let Some(f) = fixture("legacy-async") else { return };
    let prepared = f.svc.prepare_debug(debug_request(&f, SessionLease::Owned, AttachMode::LegacyAsync)).await.unwrap();
    assert_eq!(prepared.attach_mode, AttachMode::LegacyAsync);
    assert!(
        prepared.lldb.attach_commands.iter().any(|c| c.starts_with(&format!("process attach --pid {}", prepared.pid)))
    );
    assert!(lldb::uses_python(&prepared.lldb.attach_commands), "the M1 sequence toggles async mode through Python");
    let t = run_batch(&f, &prepared, BatchEnd::Kill).await;
    let stop = t.stop.clone().expect("a stop");
    assert_eq!((stop.reason.as_str(), stop.frame.line), ("breakpoint", Some(f.line)));
    assert!(t.commands[0].output.contains("= 42"), "{:?}", t.commands);
    assert!(common::eventually(Duration::from_secs(5), || !process_alive(prepared.pid)).await);
    prepared.session.terminate(TerminateMode::Kill).await.unwrap();
}
