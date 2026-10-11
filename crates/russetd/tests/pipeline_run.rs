// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The run pipeline with a fake builder, provisioning, signer and device (PLAN §4.7): build → provision → sign
//! into `.russet/build/<profile>-iphoneos/signed/` → install-if-changed → launch, and `debug.prepare` up to the
//! broker's pre-attach, plus `run.list`, `run.output.query`, `run.stop` and the `run.subscribe` forwarder.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_core::Result;
use russet_core::error::codes;
use russet_core::progress::{EventSink, JobEventKind, LogStream, ProgressSink};
use russet_debug::TerminateMode;
use russet_debug::launch::{LaunchSpec, Launched};
use russet_debug::{BoxStream, DebugConfig, DebugService, DeviceServices, async_trait};
use russet_device::{DeviceSelector, Udid};
use russet_protocol::common::{AttachMode, DebuggerState, SessionLease, SessionState};
use russet_protocol::debug::{DebugPrepareParams, DebuggerFlavor};
use russet_protocol::run::{
    InstallMode, LaunchMode, OutputFilter, RunOutputQueryParams, RunStartParams, RunStopParams,
};
use russet_toolchain::{Component, Provenance, ToolInfo};
use russetd::pipeline::ledger::InstallLedger;
use russetd::pipeline::live::copy_app;
use russetd::pipeline::run::{
    AppSigner, BuildSpec, BuiltProduct, LldbResolver, Pipeline, ProjectBuilder, Provisioned, Provisioning, SignedApp,
    signed_dir,
};
use russetd::pipeline::subscribe::{RunNotification, forward_session};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const UDID: &str = "00008101-000000000000001E";
const PID: u32 = 932;

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "russet-pl-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Sink {
    lines: Mutex<Vec<String>>,
}

impl ProgressSink for Sink {
    fn begin(&self, _t: &str, _c: bool) {}
    fn report(&self, msg: Option<&str>, _pct: Option<u8>) {
        if let Some(m) = msg {
            self.lines.lock().unwrap().push(m.to_owned());
        }
    }
    fn end(&self, _m: Option<&str>) {}
    fn log(&self, _s: LogStream, _line: &str) {}
}

impl EventSink for Sink {
    fn emit(&self, _e: JobEventKind) {}
}

/// Writes `<project>/.russet/build/debug-iphoneos/Probe.app/Probe` with `content`.
struct FakeBuilder {
    content: Mutex<Vec<u8>>,
    builds: AtomicUsize,
}

#[async_trait]
impl ProjectBuilder for FakeBuilder {
    async fn build(
        &self,
        spec: &BuildSpec,
        _sink: &dyn EventSink,
        _cancel: &CancellationToken,
    ) -> Result<BuiltProduct> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        let app = spec.project.join(".russet/build/debug-iphoneos/Probe.app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("Probe"), &*self.content.lock().unwrap()).unwrap();
        std::fs::write(app.join("Info.plist"), b"<plist/>").unwrap();
        Ok(BuiltProduct {
            executable: app.join("Probe"),
            app,
            bundle_id: "dev.russet.test.probe".into(),
            profile: "debug".into(),
            sdk_path: Some(PathBuf::from("/sdk/iPhoneOS27.0.sdk")),
        })
    }
}

struct FakeProvisioning {
    mutations: usize,
}

#[async_trait]
impl Provisioning for FakeProvisioning {
    async fn provision(&self, _product: &BuiltProduct, udid: &str) -> Result<Provisioned> {
        assert_eq!(udid, UDID);
        Ok(Provisioned {
            profiles: BTreeMap::from([(String::new(), PathBuf::from("/profiles/dev.mobileprovision"))]),
            team_id: Some("TEAMID0000".into()),
            mutations: self.mutations,
        })
    }
}

/// A thin arm64 Mach-O: `code`, then an embedded signature whose code directory (SHA-256 type) holds `slots`
/// and whose CMS blob holds `cms`. What W6's signer writes, as far as the install ledger can tell.
fn signed_macho(code: &[u8], slots: &[u8], cms: &[u8]) -> Vec<u8> {
    let be = |out: &mut Vec<u8>, words: &[u32]| words.iter().for_each(|w| out.extend_from_slice(&w.to_be_bytes()));
    let ident = b"dev.russet.test.probe\0";
    let cd_len = 44 + ident.len() + slots.len();
    let mut cd = Vec::new();
    be(&mut cd, &[0xFADE_0C02, cd_len as u32, 0x0002_0001, 0, (44 + ident.len()) as u32, 44, 0, 1, code.len() as u32]);
    cd.extend_from_slice(&[32, 2, 0, 12]); // hashSize, hashType SHA-256, platform, pageSize
    be(&mut cd, &[0]);
    cd.extend_from_slice(ident);
    cd.extend_from_slice(slots);
    let mut wrapper = Vec::new();
    be(&mut wrapper, &[0xFADE_0B01, 8 + cms.len() as u32]);
    wrapper.extend_from_slice(cms);
    let mut sig = Vec::new();
    let total = 12 + 16 + cd.len() + wrapper.len();
    be(&mut sig, &[0xFADE_0CC0, total as u32, 2, 0, 28, 0x10000, (28 + cd.len()) as u32]);
    sig.extend_from_slice(&cd);
    sig.extend_from_slice(&wrapper);
    let sig_at = (32 + 16 + code.len()).next_multiple_of(16);
    let mut out = Vec::new();
    // mach_header_64 (arm64, MH_EXECUTE, one load command), then LC_CODE_SIGNATURE.
    for w in [0xFEED_FACF_u32, 0x0100_000C, 0, 2, 1, 16, 0, 0, 0x1D, 16, sig_at as u32, sig.len() as u32] {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(code);
    out.resize(sig_at, 0);
    out.extend_from_slice(&sig);
    out
}

/// Copies and signs like W6's signer: the code directory follows the code, while the CMS signature carries the
/// signing time, so the file changes on every signing.
#[derive(Default)]
struct FakeSigner {
    signings: AtomicU32,
}

#[async_trait]
impl AppSigner for FakeSigner {
    async fn sign(&self, product: &BuiltProduct, provisioned: &Provisioned, dest: &Path) -> Result<SignedApp> {
        assert!(provisioned.profiles.contains_key(""));
        copy_app(&product.app, dest)?;
        let exe = dest.join("Probe");
        let code = std::fs::read(&exe).unwrap();
        let n = self.signings.fetch_add(1, Ordering::SeqCst);
        let cms = format!("CMS signingTime=2026-10-11T09:00:{n:02}Z");
        std::fs::write(&exe, signed_macho(&code, &code, cms.as_bytes())).unwrap();
        Ok(SignedApp { app: dest.to_path_buf() })
    }
}

struct FakeLldb;

#[async_trait]
impl LldbResolver for FakeLldb {
    async fn lldb(&self, _f: DebuggerFlavor) -> Result<ToolInfo> {
        Ok(ToolInfo {
            component: Component::Lldb,
            version: "21.0.0".into(),
            path: PathBuf::from("/usr/lib/swift/usr/bin/lldb"),
            provenance: Provenance::Uv,
            env: BTreeMap::from([("PYTHONHOME".to_owned(), "/pbs".to_owned())]),
            ok: true,
            problems: vec![],
        })
    }
    async fn lldb_dap(&self, _f: DebuggerFlavor) -> Result<ToolInfo> {
        Err(russet_core::Error::new(codes::toolchain::MISSING, "no lldb-dap"))
    }
}

/// A device whose app prints a heartbeat and whose debugserver is a tiny stub that accepts `vAttach`.
#[derive(Default)]
struct FakeDevice {
    installs: Mutex<Vec<PathBuf>>,
    launches: Mutex<Vec<LaunchSpec>>,
    signals: Mutex<Vec<(u32, i32)>>,
    stub: Mutex<Option<std::net::SocketAddr>>,
    stub_log: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl DeviceServices for FakeDevice {
    async fn resolve(&self, sel: &DeviceSelector) -> Result<Udid> {
        match sel {
            DeviceSelector::Udid(u) => Ok(u.clone()),
            _ => Ok(Udid(UDID.into())),
        }
    }

    async fn install(
        &self,
        _udid: &Udid,
        app: &Path,
        _p: &dyn ProgressSink,
    ) -> Result<russet_debug::launch::InstalledAppInfo> {
        assert!(app.join("Probe").exists());
        self.installs.lock().unwrap().push(app.to_path_buf());
        Ok(russet_debug::launch::InstalledAppInfo {
            bundle_id: "dev.russet.test.probe".into(),
            path: None,
            executable: None,
        })
    }

    async fn launch(&self, _udid: &Udid, spec: &LaunchSpec) -> Result<Launched> {
        self.launches.lock().unwrap().push(spec.clone());
        let (mut tx, rx) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            for seq in 0..5 {
                let _ = tx.write_all(format!("PROBE seq={seq} t={}\r\n", seq * 100).as_bytes()).await;
            }
            // Keep the stream open: the app keeps running.
            std::future::pending::<()>().await;
        });
        let mut l = Launched::pid_only(PID);
        l.executable = Some("/private/var/containers/Bundle/Application/1C2/Probe.app/Probe".into());
        l.stdio = Some(Box::new(rx));
        Ok(l)
    }

    async fn connect_debugserver(&self, _udid: &Udid) -> Result<BoxStream> {
        let addr = self.stub.lock().unwrap().unwrap();
        Ok(Box::new(tokio::net::TcpStream::connect(addr).await.unwrap()))
    }

    async fn signal(&self, _udid: &Udid, pid: u32, sig: i32) -> Result<()> {
        self.signals.lock().unwrap().push((pid, sig));
        Ok(())
    }
}

fn frame(p: &str) -> Vec<u8> {
    let cs = p.bytes().fold(0u8, |a, b| a.wrapping_add(b));
    format!("${p}#{cs:02x}").into_bytes()
}

/// A debugserver stub: handshake, `QSetDetachOnError`, `vAttach;3a4` → `T11`, `k` → `X09`.
async fn start_stub(log: Arc<Mutex<Vec<String>>>) -> std::net::SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    while matches!(buf.first(), Some(b'+' | b'-')) {
                        buf.remove(0);
                    }
                    if let Some(h) = buf.iter().position(|&b| b == b'#')
                        && buf.len() >= h + 3
                    {
                        let p = String::from_utf8_lossy(&buf[1..h]).into_owned();
                        buf.drain(..h + 3);
                        log.lock().unwrap().push(p.clone());
                        let reply = match p.as_str() {
                            "QStartNoAckMode" => "OK".to_owned(),
                            q if q.starts_with("qSupported") => "PacketSize=20000;qEcho+".to_owned(),
                            "qHostInfo" => "ostype:ios;vm-page-size:16384;os_version:27.0.0;".to_owned(),
                            "vAttach;3a4" => "T11thread:1a2b;threads:1a2b;reason:signal;".to_owned(),
                            "k" => "X09".to_owned(),
                            _ => "OK".to_owned(),
                        };
                        let _ = s.write_all(b"+").await;
                        let _ = s.write_all(&frame(&reply)).await;
                        if p == "k" {
                            return;
                        }
                        continue;
                    }
                    match s.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
            });
        }
    });
    addr
}

struct Fixture {
    _dir: Scratch,
    project: PathBuf,
    dev: Arc<FakeDevice>,
    builder: Arc<FakeBuilder>,
    pipeline: Pipeline,
}

async fn fixture(mutations: usize) -> Fixture {
    fixture_with(mutations, russet_debug::broker::HANDOFF_TIMEOUT).await
}

async fn fixture_with(mutations: usize, handoff: Duration) -> Fixture {
    let dir = Scratch::new("run");
    let project = dir.0.join("RussetProbe");
    std::fs::create_dir_all(&project).unwrap();
    let dev = Arc::new(FakeDevice::default());
    let stub = start_stub(Arc::clone(&dev.stub_log)).await;
    *dev.stub.lock().unwrap() = Some(stub);
    let cfg = DebugConfig {
        runtime_dir: dir.0.join("runtime"),
        state_dir: Some(dir.0.join("state")),
        lease_grace: Duration::from_millis(100),
        broker: russet_debug::broker::BrokerOptions { handoff_timeout: Some(handoff), ..Default::default() },
        ..DebugConfig::default()
    };
    let debug = Arc::new(DebugService::new(dev.clone(), cfg));
    let builder =
        Arc::new(FakeBuilder { content: Mutex::new(b"\xcf\xfa\xed\xfe v1".to_vec()), builds: AtomicUsize::new(0) });
    let pipeline = Pipeline {
        builder: builder.clone(),
        provisioning: Arc::new(FakeProvisioning { mutations }),
        signer: Arc::new(FakeSigner::default()),
        lldb: Arc::new(FakeLldb),
        debug,
        ledger: Arc::new(InstallLedger::new(&dir.0.join("state"))),
    };
    Fixture { _dir: dir, project, dev, builder, pipeline }
}

fn params(f: &Fixture) -> RunStartParams {
    RunStartParams { project: f.project.to_string_lossy().into_owned(), ..RunStartParams::default() }
}

#[tokio::test(flavor = "multi_thread")]
async fn run_builds_signs_installs_if_changed_and_launches() {
    let f = fixture(0).await;
    let sink = Sink::default();
    let cancel = CancellationToken::new();
    let mut p = params(&f);
    p.args = Some(vec!["-RussetProbeCrash".into(), "0".into()]);
    p.env = Some(BTreeMap::from([("FOO".to_owned(), "1".to_owned())]));
    let (r, session) = f.pipeline.start(p.clone(), &sink, &cancel).await.unwrap();
    assert!(r.installed);
    assert_eq!(r.bundle_id, "dev.russet.test.probe");
    assert_eq!(r.pid, Some(PID));
    let session = session.unwrap();
    assert_eq!(Some(session.id().clone()), r.session_id);
    // The signed copy is where ARCHITECTURE §14.8 puts it, and the unsigned product stays.
    let signed = signed_dir(&f.project, "debug").join("Probe.app");
    assert_eq!(f.dev.installs.lock().unwrap().as_slice(), std::slice::from_ref(&signed));
    let first_signing = std::fs::read(signed.join("Probe")).unwrap();
    assert!(first_signing.windows(4).any(|w| w == b"CMS "));
    assert_eq!(
        std::fs::read(f.project.join(".russet/build/debug-iphoneos/Probe.app/Probe")).unwrap(),
        b"\xcf\xfa\xed\xfe v1"
    );
    // The ledger records the signed executable's CDHash (ARCHITECTURE §14.8).
    let cdhash = russet_debug::macho::identity(&signed.join("Probe")).unwrap().unwrap().cdhash.unwrap();
    assert_eq!(cdhash.len(), 40);
    let ledger: serde_json::Value = serde_json::from_slice(
        &std::fs::read(f._dir.0.join("state/devices").join(UDID).join("installs.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(ledger["dev.russet.test.probe"]["cdhash"], cdhash.as_str());
    // The launch got the user's args and env plus Russet's defaults.
    let launch = f.dev.launches.lock().unwrap()[0].clone();
    assert_eq!(launch.args, ["-RussetProbeCrash", "0"]);
    assert_eq!(launch.env.get("FOO").map(String::as_str), Some("1"));
    assert_eq!(launch.env.get("NSUnbufferedIO").map(String::as_str), Some("YES"));
    assert!(!launch.suspended && launch.kill_existing);
    let stages = sink.lines.lock().unwrap().clone();
    for s in ["resolve: started", "sign: finished", "install: finished", "launch: finished"] {
        assert!(stages.iter().any(|l| l == s), "{s} missing in {stages:?}");
    }

    // The output: stdout lines through run.output.query.
    let q = RunOutputQueryParams {
        session_id: session.id().clone(),
        after_seq: None,
        since_ms: None,
        limit: None,
        filter: None,
    };
    let mut entries = Vec::new();
    for _ in 0..100 {
        entries = f.pipeline.output_query(&q).unwrap().entries;
        if entries.len() >= 5 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[0].message, "PROBE seq=0 t=0");
    assert_eq!(entries[0].process.as_deref(), Some("Probe"));
    // Followers page by number: `nextSeq` continues after the last entry.
    let page = f.pipeline.output_query(&q).unwrap();
    assert_eq!(page.next_seq, 5);
    let rest = RunOutputQueryParams { after_seq: Some(3), ..q.clone() };
    let tail: Vec<String> = f.pipeline.output_query(&rest).unwrap().entries.into_iter().map(|e| e.message).collect();
    assert_eq!(tail, ["PROBE seq=3 t=300", "PROBE seq=4 t=400"]);
    assert!(
        f.pipeline.output_query(&RunOutputQueryParams { after_seq: Some(5), ..q.clone() }).unwrap().entries.is_empty()
    );
    let filtered = RunOutputQueryParams {
        limit: Some(2),
        filter: Some(OutputFilter { contains: Some("seq=4".into()), ..OutputFilter::default() }),
        ..q.clone()
    };
    assert_eq!(f.pipeline.output_query(&filtered).unwrap().entries.len(), 1);

    // A second run with no change skips the install (the ledger): the re-signed file differs (a new signing
    // time) but its code directory does not. `always` installs; a change installs.
    let (r2, s2) = f.pipeline.start(p.clone(), &sink, &cancel).await.unwrap();
    assert_ne!(std::fs::read(signed.join("Probe")).unwrap(), first_signing, "re-signing rewrote the CMS blob");
    assert!(!r2.installed);
    let (r3, s3) = f
        .pipeline
        .start(RunStartParams { install: Some(InstallMode::Always), ..p.clone() }, &sink, &cancel)
        .await
        .unwrap();
    assert!(r3.installed);
    *f.builder.content.lock().unwrap() = b"\xcf\xfa\xed\xfe v2".to_vec();
    let (r4, s4) = f.pipeline.start(p.clone(), &sink, &cancel).await.unwrap();
    assert!(r4.installed);
    assert_eq!(f.dev.installs.lock().unwrap().len(), 3);
    assert_eq!(f.builder.builds.load(Ordering::SeqCst), 4);

    // run.list shows the wire summaries; run.stop kills through the device.
    let list = f.pipeline.list();
    assert_eq!(list.sessions.len(), 4);
    assert!(list.sessions.iter().all(|s| s.state == SessionState::Running && s.debugger == DebuggerState::None));
    assert_eq!(list.sessions[0].lease, SessionLease::Owned);
    assert_eq!(list.sessions[0].project.as_deref(), Some(f.project.to_str().unwrap()));
    for h in [&session, s2.as_ref().unwrap(), s3.as_ref().unwrap(), s4.as_ref().unwrap()] {
        f.pipeline.stop(&RunStopParams { session_id: h.id().clone(), mode: None }).await.unwrap();
    }
    assert_eq!(f.dev.signals.lock().unwrap().len(), 4);
    assert!(
        f.pipeline
            .list()
            .sessions
            .iter()
            .all(|s| s.state == SessionState::Exited && s.exit.as_ref().and_then(|e| e.signal) == Some(9))
    );
    let unknown = RunStopParams { session_id: russet_debug::new_session_id(), mode: None };
    assert_eq!(f.pipeline.stop(&unknown).await.unwrap_err().code(), codes::debug::SESSION_NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn launch_none_installs_without_a_session_and_no_build_is_passed_on() {
    let f = fixture(0).await;
    let p = RunStartParams { launch: Some(LaunchMode::None), build: Some(false), ..params(&f) };
    let (r, session) = f.pipeline.start(p, &Sink::default(), &CancellationToken::new()).await.unwrap();
    assert!(r.installed && r.session_id.is_none() && r.pid.is_none() && session.is_none());
    assert!(f.dev.launches.lock().unwrap().is_empty());
    assert!(f.pipeline.list().sessions.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn wave_1_refuses_provisioning_mutations_and_bad_projects() {
    let f = fixture(2).await;
    let e = f.pipeline.start(params(&f), &Sink::default(), &CancellationToken::new()).await.unwrap_err();
    assert_eq!(e.code(), codes::perm::APPROVAL_REQUIRED);
    assert!(f.dev.installs.lock().unwrap().is_empty());
    let bad = RunStartParams { project: "relative/dir".into(), ..RunStartParams::default() };
    let e = f.pipeline.start(bad, &Sink::default(), &CancellationToken::new()).await.unwrap_err();
    assert_eq!(e.code(), codes::config::PROJECT_NOT_FOUND);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let f = fixture(0).await;
    let e = f.pipeline.start(params(&f), &Sink::default(), &cancel).await.unwrap_err();
    assert_eq!(e.code(), codes::core::CANCELLED);
    assert_eq!(f.builder.builds.load(Ordering::SeqCst), 0);
}

// The broker endpoint is a Unix socket here; Windows uses loopback TCP.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn debug_prepare_launches_suspended_and_pre_attaches_the_broker() {
    let f = fixture(0).await;
    let p = DebugPrepareParams {
        run: RunStartParams { session_lease: Some(SessionLease::Detached), ..params(&f) },
        ..DebugPrepareParams::default()
    };
    let (r, session) = f.pipeline.prepare_debug(p, &Sink::default(), &CancellationToken::new()).await.unwrap();
    assert_eq!(r.pid, PID);
    assert_eq!(r.attach_mode, AttachMode::Broker);
    assert!(f.dev.launches.lock().unwrap()[0].suspended, "debug launches are suspended");
    // The broker owns the attach: QSetDetachOnError:0 before vAttach (RD §6.2).
    let stub = f.dev.stub_log.lock().unwrap().clone();
    let pos = |p: &str| stub.iter().position(|x| x == p).unwrap();
    assert!(pos("QSetDetachOnError:0") < pos("vAttach;3a4"), "{stub:?}");
    // The plan (RD §9.1): remote-ios, the unsigned product, search paths onto the signed copy, the broker URL.
    assert!(r.broker.lldb_url.starts_with("unix-connect:///"));
    assert!(std::path::Path::new(r.broker.path.as_deref().unwrap()).exists());
    let unsigned = f.project.join(".russet/build/debug-iphoneos/Probe.app/Probe");
    assert!(r.attach_commands.contains(&"platform select remote-ios".to_owned()));
    assert!(r.attach_commands.contains(&format!("target create --arch arm64-apple-ios \"{}\"", unsigned.display())));
    assert!(r.attach_commands.iter().any(|c| {
        c.starts_with("target modules search-paths add \"/private/var/containers/Bundle/Application/1C2/Probe.app\"")
    }));
    assert_eq!(r.attach_commands.last().unwrap(), &format!("process connect -p gdb-remote \"{}\"", r.broker.lldb_url));
    assert!(r.init_commands.iter().any(|c| c == "settings set target.sdk-path \"/sdk/iPhoneOS27.0.sdk\""));
    assert!(!russet_debug::lldb::uses_python(&r.attach_commands) && !russet_debug::lldb::uses_python(&r.init_commands));
    // attach.lldb under <project>/.russet/debug/<sid>/.
    let script = PathBuf::from(r.attach_script.clone().unwrap());
    assert_eq!(script, f.project.join(".russet/debug").join(r.session_id.as_str()).join("attach.lldb"));
    let text = std::fs::read_to_string(&script).unwrap();
    assert!(text.contains("process connect -p gdb-remote \"unix-connect:///"));
    // The tools: lldb with its env; lldb-dap reported missing rather than failing the call.
    assert_eq!(r.lldb.env.get("PYTHONHOME").map(String::as_str), Some("/pbs"));
    assert_eq!(serde_json::to_value(&r.lldb).unwrap()["component"], "lldb");
    assert_eq!(serde_json::to_value(&r.lldb).unwrap()["provenance"], serde_json::json!({"kind": "uv"}));
    assert!(!r.lldb_dap.ok && r.lldb_dap.problems[0].contains("no lldb-dap"));
    assert_eq!(r.dap_config["request"], "attach");
    assert!(r.dap_config["attachCommands"][0].as_str().unwrap().starts_with('!'));
    // The session waits for its debugger.
    let s = session.summary();
    assert_eq!((s.state, s.lease, s.debugger), (SessionState::Ready, SessionLease::Detached, DebuggerState::None));
    // run.stop kills through the broker.
    f.pipeline.stop(&RunStopParams { session_id: r.session_id.clone(), mode: None }).await.unwrap();
    assert!(f.dev.stub_log.lock().unwrap().iter().any(|p| p == "k"));
    assert_eq!(session.summary().state, SessionState::Exited);
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribers_get_state_output_and_the_end() {
    let f = fixture(0).await;
    let (_, session) = f.pipeline.start(params(&f), &Sink::default(), &CancellationToken::new()).await.unwrap();
    let session = session.unwrap();
    let seen: Arc<Mutex<Vec<RunNotification>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let sub = russet_debug::session::prefixed_ulid("sub_");
    let task = tokio::spawn(forward_session(
        session.clone(),
        sub,
        move |n| {
            sink.lock().unwrap().push(n);
            true
        },
        std::future::pending(),
    ));
    let count_lines = |seen: &Mutex<Vec<RunNotification>>| {
        seen.lock()
            .unwrap()
            .iter()
            .map(|n| if let RunNotification::Output(o) = n { o.entries.len() } else { 0 })
            .sum::<usize>()
    };
    for _ in 0..200 {
        if count_lines(&seen) >= 5 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The lines may have arrived before the subscription; the query covers those. Then the end.
    f.pipeline.stop(&RunStopParams { session_id: session.id().clone(), mode: None }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
    let seen = seen.lock().unwrap().clone();
    assert!(matches!(&seen[0], RunNotification::State(s) if s.state == SessionState::Running));
    match seen.last().unwrap() {
        RunNotification::Exited(e) => {
            assert_eq!(e.state, Some(SessionState::Exited));
            assert_eq!(e.signal, Some(9));
        }
        other => panic!("{other:?}"),
    }
}

/// An app that runs quietly; the test writes its output into the session.
struct QuietDevice;

#[async_trait]
impl DeviceServices for QuietDevice {
    async fn resolve(&self, _sel: &DeviceSelector) -> Result<Udid> {
        Ok(Udid(UDID.into()))
    }

    async fn launch(&self, _udid: &Udid, _spec: &LaunchSpec) -> Result<Launched> {
        Ok(Launched::pid_only(PID))
    }

    async fn connect_debugserver(&self, _udid: &Udid) -> Result<BoxStream> {
        unreachable!("not debugged")
    }

    async fn signal(&self, _udid: &Udid, _pid: u32, _sig: i32) -> Result<()> {
        Ok(())
    }
}

/// `run.subscribe`'s forwarder: output in batches of at most 200 entries, in order; a subscriber that falls more
/// than the session channel's 1024 entries behind loses the oldest ones and the next `run.output` says how many
/// (`dropped`), so every entry is delivered or counted. On a current-thread runtime the test pushes a burst the
/// forwarder cannot keep up with, because it cannot run until the test yields.
#[tokio::test]
async fn the_forwarder_batches_output_and_reports_entries_it_fell_behind_on() {
    const LINES: usize = 3000;
    let svc = DebugService::new(Arc::new(QuietDevice), DebugConfig::default());
    let mut req = russet_debug::RunRequest::new(DeviceSelector::Only, "dev.russet.test.probe");
    req.lease = SessionLease::Detached;
    let session = svc.run(req).await.unwrap();
    let seen: Arc<Mutex<Vec<RunNotification>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let subscribed = Arc::new(tokio::sync::Notify::new());
    let ready = Arc::clone(&subscribed);
    let task = tokio::spawn(forward_session(
        session.clone(),
        russet_debug::session::prefixed_ulid("sub_"),
        move |n| {
            // The forwarder subscribes to the output before it sends the state.
            if matches!(n, RunNotification::State(_)) {
                ready.notify_one();
            }
            sink.lock().unwrap().push(n);
            true
        },
        std::future::pending(),
    ));
    subscribed.notified().await;
    let view = session.logs();
    for i in 0..LINES {
        view.push(russet_debug::logs::stdio_entry(
            russet_protocol::common::LogSource::Stdout,
            format!("line {i}"),
            Some(PID),
            None,
        ));
    }
    let accounted = |seen: &Mutex<Vec<RunNotification>>| {
        seen.lock()
            .unwrap()
            .iter()
            .map(|n| match n {
                RunNotification::Output(o) => o.entries.len() as u64 + o.dropped.unwrap_or(0),
                _ => 0,
            })
            .sum::<u64>()
    };
    for _ in 0..500 {
        if accounted(&seen) >= LINES as u64 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    session.terminate(TerminateMode::Kill).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
    let seen = seen.lock().unwrap().clone();
    let outputs: Vec<&russet_protocol::run::RunOutputParams> =
        seen.iter().filter_map(|n| if let RunNotification::Output(o) = n { Some(o) } else { None }).collect();
    assert!(outputs.iter().all(|o| o.entries.len() <= russetd::pipeline::subscribe::OUTPUT_BATCH));
    assert!(outputs.len() >= 1024 / russetd::pipeline::subscribe::OUTPUT_BATCH, "{} batches", outputs.len());
    let dropped: u64 = outputs.iter().filter_map(|o| o.dropped).sum();
    assert_eq!(dropped, (LINES - 1024) as u64, "the oldest entries beyond the channel are counted, not lost silently");
    let numbers: Vec<usize> = outputs
        .iter()
        .flat_map(|o| o.entries.iter())
        .map(|e| e.message.strip_prefix("line ").unwrap().parse().unwrap())
        .collect();
    assert_eq!(numbers.len() as u64 + dropped, LINES as u64, "every entry is delivered or counted");
    assert!(numbers.windows(2).all(|w| w[0] < w[1]), "in order, no duplicates");
    assert_eq!(numbers.last(), Some(&(LINES - 1)), "the newest entries arrive");
    assert!(matches!(seen.last(), Some(RunNotification::Exited(_))));
}

/// A debug session nobody attaches to fails with `debug.handoffTimeout`; `run.exited` says so.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn run_exited_carries_the_reason_a_session_failed() {
    let f = fixture_with(0, Duration::from_millis(300)).await;
    let p = DebugPrepareParams {
        run: RunStartParams { session_lease: Some(SessionLease::Detached), ..params(&f) },
        ..DebugPrepareParams::default()
    };
    let (_, session) = f.pipeline.prepare_debug(p, &Sink::default(), &CancellationToken::new()).await.unwrap();
    let seen: Arc<Mutex<Vec<RunNotification>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let sub = russet_debug::session::prefixed_ulid("sub_");
    let forward = forward_session(
        session.clone(),
        sub,
        move |n| {
            sink.lock().unwrap().push(n);
            true
        },
        std::future::pending(),
    );
    tokio::time::timeout(Duration::from_secs(10), forward).await.unwrap();
    let seen = seen.lock().unwrap().clone();
    let Some(RunNotification::Exited(e)) = seen.last() else { panic!("{seen:?}") };
    assert_eq!(e.state, Some(SessionState::Failed));
    let err = e.error.clone().unwrap();
    assert_eq!((err.code, err.russet_code.as_str()), (6007, "debug.handoffTimeout"));
    assert!(err.hint.is_some());
    // The broker stopped the app it held.
    assert!(f.dev.stub_log.lock().unwrap().iter().any(|p| p == "k"));
}
