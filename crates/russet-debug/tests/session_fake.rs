// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Sessions over a scripted device (no toolchain needed): console entries from the device plane, the end of the
//! console as the exit when there is no exit notice, a debug session against the fake iOS debugserver from
//! `debug.prepare` to LLDB's `D`, and a failing attach that leaves no stopped app behind.

// Unix sockets, /proc and lldb-server: these harnesses run on Linux (and macOS) hosts.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::FakeDevice;
use russet_core::Result;
use russet_debug::launch::{LaunchSpec, Launched};
use russet_debug::lldb::{InitSettings, LldbPlatform};
use russet_debug::{
    AttachMode, BoxStream, DebugConfig, DebugRequest, DebugService, DebugTarget, DebuggerState, DeviceServices,
    Frontend, RunRequest, SessionLease, SessionState, TerminateMode, WireSessionState, async_trait,
};
use russet_device::{DeviceSelector, Udid};
use russet_gdbremote::{GdbClient, HandshakeOptions};
use russet_protocol::common::{LogEntry, LogLevel, LogSource};
use russet_testkit::debug::{FakeDebugserver, FakeImage, FakeSpec, macho};

/// A device whose console arrives as entries and ends after three lines.
struct EntriesDevice;

#[async_trait]
impl DeviceServices for EntriesDevice {
    async fn resolve(&self, _sel: &DeviceSelector) -> Result<Udid> {
        Ok(Udid("u".into()))
    }

    async fn launch(&self, _udid: &Udid, _spec: &LaunchSpec) -> Result<Launched> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            for i in 0..3 {
                let e = LogEntry {
                    ts: russet_debug::logs::now_rfc3339(),
                    source: LogSource::Stdout,
                    level: LogLevel::Default,
                    message: format!("PROBE seq={i}"),
                    pid: Some(7),
                    tid: None,
                    process: Some("RussetProbe".into()),
                    subsystem: None,
                    category: None,
                    image_uuid: None,
                    image_offset: None,
                    mach_ts: None,
                };
                let _ = tx.send(e).await;
            }
        });
        let mut l = Launched::pid_only(7);
        l.entries = Some(rx);
        Ok(l)
    }

    async fn connect_debugserver(&self, _udid: &Udid) -> Result<BoxStream> {
        unreachable!("not debugged")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn console_entries_feed_the_output_and_their_end_ends_the_session() {
    let svc = DebugService::new(Arc::new(EntriesDevice), DebugConfig::default());
    let s = svc.run(RunRequest::new(DeviceSelector::Only, "dev.russet.test.probe")).await.unwrap();
    let end = s.wait_for(Duration::from_secs(5), SessionState::is_final).await;
    assert!(matches!(end, SessionState::Exited { .. }), "{end:?}");
    let (entries, _) = s.logs().query(None, None, &Default::default());
    let msgs: Vec<&str> = entries.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(msgs, ["PROBE seq=0", "PROBE seq=1", "PROBE seq=2"]);
    assert!(s.summary().exit.is_some_and(|e| e.code.is_none() && e.signal.is_none()), "unknown status");
}

fn fake_device(pid: u32) -> Arc<FakeDevice> {
    let fake = FakeDebugserver::start(FakeSpec {
        pid,
        image: Some(FakeImage {
            device_path: "/private/var/containers/Bundle/Application/1C2/RussetProbe.app/RussetProbe".into(),
            macho: macho::synthetic_ios_executable([9; 16]),
        }),
        ..FakeSpec::default()
    })
    .unwrap();
    Arc::new(FakeDevice {
        fake,
        pid: 932,
        executable: "/private/var/containers/Bundle/Application/1C2/RussetProbe.app/RussetProbe".into(),
        signals: Mutex::new(Vec::new()),
        launches: Mutex::new(Vec::new()),
    })
}

fn request(lease: SessionLease) -> DebugRequest {
    let mut run = RunRequest::new(DeviceSelector::Only, "dev.russet.test.probe");
    run.lease = lease;
    DebugRequest {
        run,
        stop_on_entry: false,
        frontend: Frontend::LldbCli,
        attach_mode: AttachMode::Broker,
        lldb: None,
        lldb_dap: None,
        target: DebugTarget {
            local_executable: "/p/.russet/build/debug-iphoneos/RussetProbe.app/RussetProbe".into(),
            local_app_dir: Some("/p/.russet/build/debug-iphoneos/signed/RussetProbe.app".into()),
            sdk_path: None,
            arch: Some("arm64-apple-ios".into()),
        },
        platform: LldbPlatform::RemoteIos { sysroot: None },
        init: InitSettings::default(),
        script_root: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_debug_session_from_prepare_to_lldb_detach() {
    let dir = common::scratch("session-fake");
    let dev = fake_device(932);
    let cfg =
        DebugConfig { runtime_dir: dir.join("run"), state_dir: Some(dir.join("state")), ..DebugConfig::default() };
    let svc = DebugService::new(dev.clone(), cfg);
    let prepared = svc.prepare_debug(request(SessionLease::Detached)).await.unwrap();
    let s = prepared.session.clone();
    assert!(dev.launches.lock().unwrap()[0].suspended);
    assert_eq!(s.summary().state, WireSessionState::Ready);
    // The plan maps the device app onto the signed copy; the script is under state/sessions/<sid>/.
    assert!(prepared.lldb.attach_commands.contains(
        &"target modules search-paths add \"/private/var/containers/Bundle/Application/1C2/RussetProbe.app\" \"/p/.russet/build/debug-iphoneos/signed/RussetProbe.app\"".to_owned()
    ));
    let script = prepared.attach_script.clone().unwrap();
    assert_eq!(script, dir.join("state/sessions").join(s.id().as_str()).join("attach.lldb"));
    assert_eq!(prepared.lldb.program, std::path::PathBuf::from("lldb"));

    // "LLDB" connects through the broker: the debugger is attached and the app stopped.
    let russet_debug::BrokerEndpoint::Unix { path } = &prepared.broker else { panic!("unix expected") };
    let stream: BoxStream = Box::new(tokio::net::UnixStream::connect(path).await.unwrap());
    let lldb = GdbClient::handshake(stream, HandshakeOptions::default()).await.unwrap().0;
    let st = s.wait_for(Duration::from_secs(5), |st| *st == SessionState::Stopped).await;
    assert_eq!(st, SessionState::Stopped);
    assert_eq!(s.debugger(), DebuggerState::Attached);
    // `D`: the session stays `running` with `debugger: detached`; the fake detached rather than killed.
    lldb.detach().await.unwrap();
    let st = s.wait_for(Duration::from_secs(5), |_| s.debugger() == DebuggerState::Detached).await;
    assert_eq!(st, SessionState::Running);
    assert!(dev.fake.wait_for(Duration::from_secs(5), |r| r.detached && !r.killed));
    // Later kills go through the device's signal.
    s.terminate(TerminateMode::Kill).await.unwrap();
    assert_eq!(dev.signals.lock().unwrap().as_slice(), &[(932, 9)]);
    assert_eq!(s.summary().state, WireSessionState::Exited);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_attach_kills_the_suspended_app_and_fails_the_session() {
    // The fake process has another pid, so vAttach answers "no such process".
    let dev = fake_device(4242);
    let svc = DebugService::new(dev.clone(), DebugConfig::default());
    let e = svc.prepare_debug(request(SessionLease::Owned)).await.unwrap_err();
    assert_eq!(e.code(), russet_core::error::codes::debug::PROCESS_EXITED);
    assert_eq!(dev.signals.lock().unwrap().as_slice(), &[(932, 9)], "no stopped app is left behind");
    let summaries = svc.sessions();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].state, WireSessionState::Failed);
}
