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

/// A configuration whose broker sockets live in the test's scratch directory.
fn config(dir: &std::path::Path) -> DebugConfig {
    DebugConfig { runtime_dir: dir.join("run"), ..DebugConfig::default() }
}

fn request(lease: SessionLease) -> DebugRequest {
    let mut run = RunRequest::new(DeviceSelector::Only, "dev.russet.test.probe");
    run.lease = lease;
    DebugRequest {
        run,
        stop_on_entry: false,
        frontend: Frontend::LldbCli,
        attach_mode: AttachMode::Broker,
        flavor: russet_protocol::debug::DebuggerFlavor::Swift,
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
    let dir = common::scratch("sf-main");
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
    assert!(path.starts_with(dir.join("run/sessions").join(s.id().as_str())), "{}", path.display());
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
    assert!(!dir.join("run/sessions").join(s.id().as_str()).exists(), "the runtime directory goes with the session");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_attach_kills_the_suspended_app_and_fails_the_session() {
    // The fake process has another pid, so vAttach answers "no such process".
    let dir = common::scratch("sf-attach");
    let dev = fake_device(4242);
    let svc = DebugService::new(dev.clone(), config(dir.path()));
    let e = svc.prepare_debug(request(SessionLease::Owned)).await.unwrap_err();
    assert_eq!(e.code(), russet_core::error::codes::debug::PROCESS_EXITED);
    assert_eq!(dev.signals.lock().unwrap().as_slice(), &[(932, 9)], "no stopped app is left behind");
    let summaries = svc.sessions();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].state, WireSessionState::Failed);
}

/// Nobody connects within the handoff limit: the broker kills the app and the session ends `failed` with
/// `debug.handoffTimeout` (6007), which `session.json`, the output and `SessionHandle::error` report. The record
/// also carries RD §21's device, app, launch, broker and LLDB fields.
#[tokio::test(flavor = "multi_thread")]
async fn a_handoff_timeout_fails_the_session_with_its_error_and_session_json_has_rd_21s_fields() {
    let dir = common::scratch("sf-handoff");
    let dev = fake_device(932);
    let cfg = DebugConfig {
        runtime_dir: dir.join("run"),
        state_dir: Some(dir.join("state")),
        broker: russet_debug::broker::BrokerOptions {
            handoff_timeout: Some(Duration::from_millis(300)),
            ..Default::default()
        },
        ..DebugConfig::default()
    };
    let svc = DebugService::new(dev.clone(), cfg);
    let exe = dir.join("RussetProbe");
    std::fs::write(&exe, macho::synthetic_ios_executable([9; 16])).unwrap();
    let mut r = request(SessionLease::Detached);
    r.target.local_executable = exe;
    r.run.process_name = Some("RussetProbe".into());
    r.lldb = Some(russet_toolchain::ToolInfo {
        component: russet_toolchain::Component::Lldb,
        version: "21.0.0".into(),
        path: "/usr/lib/swift/usr/bin/lldb".into(),
        provenance: russet_toolchain::Provenance::System,
        env: [("PYTHONHOME".to_owned(), "/u/python/cpython-3.12.15-linux-x86_64-gnu".to_owned())].into(),
        ok: true,
        problems: vec![],
    });
    let prepared = svc.prepare_debug(r).await.unwrap();
    let s = prepared.session.clone();
    let end = s.wait_for(Duration::from_secs(10), SessionState::is_final).await;
    assert_eq!(end, SessionState::Failed);
    let err = s.error().expect("the session knows why it failed");
    assert_eq!((err.code, err.russet_code.as_str()), (6007, "debug.handoffTimeout"));
    assert!(err.message.contains("within 0 s") || err.message.contains("within"), "{}", err.message);
    assert!(err.hint.is_some());
    assert!(dev.fake.wait_for(Duration::from_secs(5), |r| r.killed), "the broker stopped the app");
    // The output says so, in a form clients read back.
    let (entries, _) = s.logs().query(None, None, &Default::default());
    let reported: Vec<_> = entries.iter().filter_map(russet_debug::logs::parse_error_entry).collect();
    assert_eq!(reported, std::slice::from_ref(&err));
    // session.json (RD §21).
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.join("state/sessions").join(s.id().as_str()).join("session.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["schema"], 1);
    assert_eq!(record["kind"], "debug");
    assert_eq!(record["device"]["productType"], "iPhone13,2");
    assert_eq!(record["device"]["productVersion"], "27.0");
    assert_eq!(record["device"]["buildVersion"], "24A437");
    assert_eq!(record["device"]["transport"], "usb");
    assert_eq!(record["app"]["bundleId"], "dev.russet.test.probe");
    assert_eq!(record["app"]["devicePath"], "/private/var/containers/Bundle/Application/1C2/RussetProbe.app");
    assert_eq!(record["app"]["executable"], "RussetProbe");
    assert_eq!(record["app"]["uuid"], "09090909-0909-0909-0909-090909090909");
    assert_eq!(record["pid"], 932);
    assert_eq!(record["launch"], "auto");
    assert!(record["broker"]["lldbUrl"].as_str().unwrap().starts_with("unix-connect:///"));
    assert_eq!(
        record["lldb"],
        serde_json::json!({"flavor": "swift", "version": "21.0.0", "python": "cpython-3.12.15-linux-x86_64-gnu"})
    );
    assert!(record["jit"].is_null());
    assert!(record["endedAt"].is_string());
    assert_eq!(record["outcome"]["state"], "failed");
    assert_eq!(record["outcome"]["error"]["russetCode"], "debug.handoffTimeout");
    assert_eq!(record["outcome"]["error"]["code"], 6007);
}

/// A device whose app console the test controls, with the fake debugserver behind it.
struct ConsoleDevice {
    fake: FakeDebugserver,
    console: Mutex<Option<tokio::io::DuplexStream>>,
}

#[async_trait]
impl DeviceServices for ConsoleDevice {
    async fn resolve(&self, _sel: &DeviceSelector) -> Result<Udid> {
        Ok(Udid("u".into()))
    }

    async fn launch(&self, _udid: &Udid, _spec: &LaunchSpec) -> Result<Launched> {
        let (app_side, russet_side) = tokio::io::duplex(1024);
        *self.console.lock().unwrap() = Some(app_side);
        let mut l = Launched::pid_only(932);
        l.stdio = Some(Box::new(russet_side));
        Ok(l)
    }

    async fn connect_debugserver(&self, _udid: &Udid) -> Result<BoxStream> {
        Ok(Box::new(tokio::net::TcpStream::connect(self.fake.addr()).await.unwrap()))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn after_a_detach_the_end_of_stdout_ends_the_session() {
    let fake = FakeDebugserver::start(FakeSpec { pid: 932, ..FakeSpec::default() }).unwrap();
    let dev = Arc::new(ConsoleDevice { fake, console: Mutex::new(None) });
    let dir = common::scratch("sf-stdout");
    let svc = DebugService::new(dev.clone(), config(dir.path()));
    let prepared = svc.prepare_debug(request(SessionLease::Detached)).await.unwrap();
    let s = prepared.session.clone();
    let russet_debug::BrokerEndpoint::Unix { path } = &prepared.broker else { panic!("unix expected") };
    assert!(path.starts_with(dir.join("run/sessions").join(s.id().as_str())), "{}", path.display());
    let stream: BoxStream = Box::new(tokio::net::UnixStream::connect(path).await.unwrap());
    let lldb = GdbClient::handshake(stream, HandshakeOptions::default()).await.unwrap().0;
    // While the broker holds the process, the console's end does not end the session.
    lldb.detach().await.unwrap();
    s.wait_for(Duration::from_secs(5), |_| s.debugger() == DebuggerState::Detached).await;
    assert!(!s.is_ended());
    {
        use tokio::io::AsyncWriteExt;
        let mut console = dev.console.lock().unwrap().take().unwrap();
        console.write_all(b"last line\r\n").await.unwrap();
    }
    let end = s.wait_for(Duration::from_secs(5), SessionState::is_final).await;
    assert!(matches!(end, SessionState::Exited { .. }), "{end:?}");
    let (entries, _) = s.logs().query(None, None, &Default::default());
    assert!(entries.iter().any(|e| e.message == "last line"));
    // The session's runtime directory (the broker socket's) goes with it.
    let session_dir = dir.join("run/sessions").join(s.id().as_str());
    assert!(!session_dir.exists(), "{} remains", session_dir.display());
}

/// A device whose launch finishes only when the test lets it.
struct SlowLaunchDevice {
    gate: tokio::sync::Notify,
    signals: Mutex<Vec<(u32, i32)>>,
}

#[async_trait]
impl DeviceServices for SlowLaunchDevice {
    async fn resolve(&self, _sel: &DeviceSelector) -> Result<Udid> {
        Ok(Udid("u".into()))
    }

    async fn launch(&self, _udid: &Udid, _spec: &LaunchSpec) -> Result<Launched> {
        self.gate.notified().await;
        Ok(Launched::pid_only(77))
    }

    async fn connect_debugserver(&self, _udid: &Udid) -> Result<BoxStream> {
        unreachable!("not debugged")
    }

    async fn signal(&self, _udid: &Udid, pid: u32, sig: i32) -> Result<()> {
        self.signals.lock().unwrap().push((pid, sig));
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_during_the_launch_kills_the_app_and_cancels_the_start() {
    let dev = Arc::new(SlowLaunchDevice { gate: tokio::sync::Notify::new(), signals: Mutex::new(Vec::new()) });
    let svc = Arc::new(DebugService::new(dev.clone(), DebugConfig::default()));
    let mut created = svc.watch_new();
    let run = tokio::spawn({
        let svc = Arc::clone(&svc);
        async move { svc.run(RunRequest::new(DeviceSelector::Only, "dev.russet.test.probe")).await }
    });
    let s = created.recv().await.unwrap();
    let st = s.wait_for(Duration::from_secs(5), |st| *st == SessionState::Launching).await;
    assert_eq!(st, SessionState::Launching);
    // `run.stop` while the device still launches: the session ends at once...
    s.terminate(TerminateMode::Kill).await.unwrap();
    assert!(s.is_ended());
    // ...and the launch, finding it ended, kills the app it just started instead of tracking it.
    dev.gate.notify_one();
    let e = run.await.unwrap().unwrap_err();
    assert_eq!(e.code(), russet_core::error::codes::core::CANCELLED);
    assert_eq!(dev.signals.lock().unwrap().as_slice(), &[(77, 9)]);
    assert_eq!(s.summary().state, WireSessionState::Exited);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_after_the_broker_serves_kills_the_app_and_closes_the_endpoint() {
    let dir = common::scratch("sf-abort");
    // The script root is a file, so writing attach.lldb fails after the broker already serves.
    let root = dir.join("not-a-directory");
    std::fs::write(&root, b"").unwrap();
    let dev = fake_device(932);
    let svc = DebugService::new(dev.clone(), config(dir.path()));
    let mut r = request(SessionLease::Detached);
    r.script_root = Some(root);
    let e = svc.prepare_debug(r).await.unwrap_err();
    assert_eq!(e.code(), russet_core::error::codes::core::IO);
    let summaries = svc.sessions();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].state, WireSessionState::Failed);
    // The broker killed the suspended app through debugserver; the device was not asked to.
    assert!(dev.fake.wait_for(Duration::from_secs(5), |r| r.attach_count == 1 && r.killed));
    assert!(dev.signals.lock().unwrap().is_empty());
    // Nothing listens for a debugger any more.
    let sock = russet_debug::broker::socket_path(&dir.join("run"), summaries[0].session_id.as_str(), "gdb.sock");
    assert!(sock.starts_with(dir.path()), "{}", sock.display());
    assert!(common::eventually(Duration::from_secs(5), || !sock.exists()).await, "{} remains", sock.display());
    assert!(!dir.join("run/sessions").join(summaries[0].session_id.as_str()).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_during_a_suspended_launch_kills_the_app_before_any_attach() {
    let dir = common::scratch("sf-stop");
    let dev = Arc::new(SlowLaunchDevice { gate: tokio::sync::Notify::new(), signals: Mutex::new(Vec::new()) });
    let svc = Arc::new(DebugService::new(dev.clone(), config(dir.path())));
    let mut created = svc.watch_new();
    let prepare = tokio::spawn({
        let svc = Arc::clone(&svc);
        async move { svc.prepare_debug(request(SessionLease::Owned)).await.map(|_| ()) }
    });
    let s = created.recv().await.unwrap();
    s.wait_for(Duration::from_secs(5), |st| *st == SessionState::Launching).await;
    s.terminate(TerminateMode::Kill).await.unwrap();
    dev.gate.notify_one();
    // The device's debugserver is never asked for (SlowLaunchDevice panics if it is).
    let e = prepare.await.unwrap().unwrap_err();
    assert_eq!(e.code(), russet_core::error::codes::core::CANCELLED);
    assert_eq!(dev.signals.lock().unwrap().as_slice(), &[(77, 9)]);
}
