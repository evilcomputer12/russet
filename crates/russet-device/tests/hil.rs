// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Hardware-in-the-loop tests (device.md §15.4; PLAN §4.4 tests): lane A only, so every test is `#[ignore]`d and a
//! plain `cargo test` lists them as ignored. Run with `--ignored`, they still refuse to touch a phone (and say
//! `SKIPPED`) unless `RUSSET_HIL_UDID` is set **and** `RUSSET_LANE=A` (PLAN §0 rule 4), and they take the host-wide
//! live-device lock first, so they never run while a live daemon owns the phone (ARCHITECTURE §3.3 item 7).
//!
//! - `RUSSET_HIL_UDID`: the device's UDID, or `only` for the only attached device;
//! - `RUSSET_HIL_BUNDLE`: an installed, debuggable app for the launch and log tests;
//! - `RUSSET_HIL_APP`: a signed `.app` of that bundle for the install tests (an `.ipa` is built from it);
//! - `RUSSET_HOME`: the lane-A home (`~/russet-lab/home-A`) whose `config.toml` the device plane reads.
//!
//! Run them serially: `RUSSET_LANE=A RUSSET_HIL_UDID=only RUSSET_HIL_BUNDLE=… cargo test -p russet-device --test hil
//! -- --ignored --test-threads=1`. Output never prints UDIDs or device names.

use std::sync::Arc;
use std::time::{Duration, Instant};

use russet_core::dirs::RussetDirs;
use russet_device::io::{FacadeCall, LiveConnector, ServiceConnector, facade};
use russet_device::logs::LogSourceKind;
use russet_device::{DeviceConfig, DeviceHandle, DeviceManager, DeviceSelector, LaunchRequest, LogFilter};
use russet_protocol::common::LogSource;

/// One HIL test at a time, even when the harness runs them in parallel.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Hil {
    manager: Arc<DeviceManager>,
    conn: Arc<LiveConnector>,
    handle: DeviceHandle,
    /// Held for the test's life; released after the manager is gone (fields drop in order).
    _lock: russet_device::live_lock::LiveDeviceLock,
}

/// The HIL harness, or `None` (with a `SKIPPED` line) when this is not a configured lane-A run.
async fn hil() -> Option<Hil> {
    let Some(udid) = std::env::var("RUSSET_HIL_UDID").ok().filter(|s| !s.is_empty()) else {
        eprintln!("SKIPPED: RUSSET_HIL_UDID is not set (lane A only; see tests/hil.rs)");
        return None;
    };
    if std::env::var("RUSSET_LANE").ok().as_deref() != Some("A") {
        eprintln!("SKIPPED: RUSSET_HIL_UDID is set but RUSSET_LANE is not A (PLAN §0 rule 4)");
        return None;
    }
    let Some(home) = std::env::var_os("RUSSET_HOME").map(std::path::PathBuf::from) else {
        eprintln!("SKIPPED: RUSSET_HOME is not set (the lane-A home, ~/russet-lab/home-A)");
        return None;
    };
    // Never next to a live daemon: take the host-wide live-device lock before any connector exists.
    let lock = russet_device::live_lock::take(&home).unwrap_or_else(|e| {
        panic!("{} {}", e.message(), e.to_data().details.map(|d| d.to_string()).unwrap_or_default())
    });
    let cfg = DeviceConfig::load(&RussetDirs::under(&home)).expect("the lane-A config");
    let conn = Arc::new(LiveConnector::new(&cfg).expect("live connector"));
    let manager = DeviceManager::start(cfg, conn.clone() as Arc<dyn ServiceConnector>).await.expect("device manager");
    let sel = if udid == "only" { DeviceSelector::Only } else { DeviceSelector::parse(Some(&udid)) };
    let handle = manager.get(&sel).expect("the HIL device is attached");
    Some(Hil { manager, conn, handle, _lock: lock })
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane A: needs the iPhone (RUSSET_LANE=A, RUSSET_HIL_UDID); run with --ignored"]
async fn hil_device_list_reports_the_identity_quickly() {
    let _serial = SERIAL.lock().await;
    let Some(h) = hil().await else { return };
    let t = Instant::now();
    let list = russet_device::rpc::device_list(&h.manager).await.expect("device.list");
    let cold = t.elapsed();
    let d = list.devices.iter().find(|d| d.udid == h.handle.udid().as_str()).expect("listed");
    assert!(!d.product_type.is_empty() && !d.os_version.is_empty(), "identity read");
    assert!(cold < Duration::from_millis(300), "device.list took {cold:?}");
    eprintln!("device.list: {} {} in {cold:?}", d.product_type, d.os_version);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane A: needs the iPhone (RUSSET_LANE=A, RUSSET_HIL_UDID); run with --ignored"]
async fn hil_readiness_and_ddi_status() {
    let _serial = SERIAL.lock().await;
    let Some(h) = hil().await else { return };
    let r = h.handle.readiness(true).await.expect("readiness");
    assert!(r.paired);
    assert_eq!(r.developer_mode, russet_device::model::Tri::Yes);
    let ddi = h.handle.ddi_status().await.expect("ddi status");
    assert!(ddi.mounted, "a DDI is mounted on the rig");
    eprintln!("ddi: {:?} {:?} {:?} source {:?}", ddi.kind, ddi.version, ddi.product_build, ddi.source);
}

/// Opens `n` sessions (a CoreDeviceProxy tunnel plus the RSD handshake), closing the tunnel before each and pausing
/// `pause` after the close; returns the open times.
async fn session_cycles(h: &Hil, n: usize, pause: Duration) -> Vec<Duration> {
    let udid = serde_json::json!({ "udid": h.handle.udid().as_str() });
    let mut times = Vec::new();
    for i in 0..n {
        h.conn.call(FacadeCall::new(facade::TUNNEL_CLOSE, udid.clone())).await.expect("close");
        tokio::time::sleep(pause).await;
        let t = Instant::now();
        h.conn
            .call(FacadeCall::new(facade::RSD_SERVICES, udid.clone()))
            .await
            .unwrap_or_else(|e| panic!("session open {i} failed: {}", e.message()));
        times.push(t.elapsed());
    }
    times.sort();
    times
}

/// DEV-04: 100 sequential session opens with 0 failures, p95 < 300 ms (device.md §15.4). The device keeps a closed
/// tunnel about 10 s and refuses new ones while about two dozen linger, so the cycles run at a pace it sustains
/// (a 350 ms pause after each close: about one tunnel per 0.6 s). "0 failures" counts device refusals the connector
/// retried and opens it had to pace, not only errors that reached the caller.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane A: needs the iPhone (RUSSET_LANE=A, RUSSET_HIL_UDID); run with --ignored"]
async fn hil_100_session_opens_without_a_failure() {
    let _serial = SERIAL.lock().await;
    let Some(h) = hil().await else { return };
    let times = session_cycles(&h, 100, Duration::from_millis(350)).await;
    let p95 = times[94];
    let (retries, waits) = (h.conn.tunnel_open_retries(), h.conn.tunnel_open_waits());
    eprintln!(
        "100 session opens: median {:?}, p95 {p95:?}, device refusals retried: {retries}, paced opens: {waits}",
        times[49]
    );
    assert!(p95 < Duration::from_millis(300), "p95 {p95:?}");
    assert_eq!(retries, 0, "the device refused {retries} tunnel open(s)");
    assert_eq!(waits, 0, "{waits} open(s) waited for the device's tunnel budget");
}

/// Back-to-back tunnel cycles (no pause) exceed what the device sustains: the connector starts at most one tunnel per
/// device per 500 ms instead of running into the device's refusals (about every 24th open otherwise), and every open
/// succeeds.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane A: needs the iPhone (RUSSET_LANE=A, RUSSET_HIL_UDID); run with --ignored"]
async fn hil_back_to_back_tunnels_are_paced_not_refused() {
    let _serial = SERIAL.lock().await;
    let Some(h) = hil().await else { return };
    // Let the tunnels of the previous test age out first.
    tokio::time::sleep(Duration::from_secs(11)).await;
    let t = Instant::now();
    let times = session_cycles(&h, 40, Duration::ZERO).await;
    let (retries, waits) = (h.conn.tunnel_open_retries(), h.conn.tunnel_open_waits());
    eprintln!(
        "40 back-to-back session opens in {:?}: median {:?}, max {:?}, device refusals retried: {retries}, paced opens: {waits}",
        t.elapsed(),
        times[19],
        times[39]
    );
    assert_eq!(retries, 0, "the device refused {retries} tunnel open(s) despite the pacing");
    assert!(waits > 0, "back-to-back opens are paced");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane A: needs the iPhone (RUSSET_LANE=A, RUSSET_HIL_UDID); run with --ignored"]
async fn hil_afc_through_the_tunnel_is_fast_both_ways() {
    use idevice::afc::AfcClient;
    use idevice::afc::opcode::AfcFopenMode;
    let _serial = SERIAL.lock().await;
    let Some(h) = hil().await else { return };
    let stream = h.handle.connect_rsd("com.apple.afc.shim.remote").await.expect("afc shim");
    let mut dev = idevice::Idevice::new(stream, "russet");
    dev.rsd_checkin().await.expect("checkin");
    let mut afc = AfcClient::new(dev);
    let mib = 32;
    let data: Vec<u8> = (0..mib * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let path = format!("/russet-hil-{}.bin", std::process::id());
    let t = Instant::now();
    {
        let mut fd = afc.open(path.as_str(), AfcFopenMode::WrOnly).await.expect("open");
        fd.write_entire(&data).await.expect("write");
        fd.close().await.expect("close");
    }
    let up = mib as f64 / t.elapsed().as_secs_f64();
    let t = Instant::now();
    let back = {
        let mut fd = afc.open(path.as_str(), AfcFopenMode::RdOnly).await.expect("open");
        let b = fd.read_entire().await.expect("read");
        fd.close().await.expect("close");
        b
    };
    let down = mib as f64 / t.elapsed().as_secs_f64();
    afc.remove(path.as_str()).await.expect("remove");
    eprintln!("AFC through the tunnel, {mib} MiB: up {up:.1} MiB/s, down {down:.1} MiB/s");
    assert!(back == data, "the file came back identical");
    assert!(up >= 20.0 && down >= 20.0, "up {up:.1} MiB/s, down {down:.1} MiB/s");
}

/// Builds `Payload/<Name>.app/…` from a signed `.app` (an .ipa without symlink care: test apps have none).
fn make_ipa(app: &std::path::Path, out: &std::path::Path) {
    let f = std::fs::File::create(out).expect("ipa");
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    let name = app.file_name().and_then(|n| n.to_str()).expect("app name").to_owned();
    let mut stack = vec![app.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rel = dir.strip_prefix(app.parent().expect("parent")).expect("rel").display().to_string();
        z.add_directory(format!("Payload/{rel}/"), opts).expect("dir");
        for e in std::fs::read_dir(&dir).expect("read_dir").flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(app.parent().expect("parent")).expect("rel").display().to_string();
                z.start_file(format!("Payload/{rel}"), opts).expect("file");
                std::io::Write::write_all(&mut z, &std::fs::read(&p).expect("read")).expect("write");
            }
        }
    }
    z.finish().expect("finish");
    let _ = name;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane A: needs the iPhone (RUSSET_LANE=A, RUSSET_HIL_UDID); run with --ignored"]
async fn hil_installs_an_app_and_an_ipa() {
    let _serial = SERIAL.lock().await;
    let Some(h) = hil().await else { return };
    let Some(app) = std::env::var_os("RUSSET_HIL_APP").map(std::path::PathBuf::from) else {
        eprintln!("skipped: RUSSET_HIL_APP is not set");
        return;
    };
    let r = h.handle.install(&app, Default::default(), &russet_device::NoProgress).await.expect("install the .app");
    eprintln!(".app installed in {:?}", r.duration);
    let tmp = tempfile::tempdir().expect("tempdir");
    let ipa = tmp.path().join("hil.ipa");
    make_ipa(&app, &ipa);
    let r2 = h.handle.install(&ipa, Default::default(), &russet_device::NoProgress).await.expect("install the .ipa");
    eprintln!(".ipa installed in {:?}", r2.duration);
    assert_eq!(r.bundle_id, r2.bundle_id);
    assert!(r2.installed_path.is_some());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane A: needs the iPhone (RUSSET_LANE=A, RUSSET_HIL_UDID); run with --ignored"]
async fn hil_suspended_launch_sigcont_console_first_logs_and_exit() {
    let _serial = SERIAL.lock().await;
    let Some(h) = hil().await else { return };
    let Some(bundle) = std::env::var("RUSSET_HIL_BUNDLE").ok() else {
        eprintln!("skipped: RUSSET_HIL_BUNDLE is not set");
        return;
    };
    // dyld's library list is console output every app produces (OS_ACTIVITY_DT_MODE does not mirror os_log to
    // stderr on iOS 27.0).
    let env = [("DYLD_PRINT_LIBRARIES".to_owned(), "1".to_owned())].into();
    let req = LaunchRequest {
        bundle_id: bundle,
        suspended: true,
        kill_existing: true,
        console: true,
        env,
        ..Default::default()
    };
    let p = h.handle.launch(req).await.expect("suspended launch");
    assert!(p.pid > 0);
    // Subscribe before resuming: logs from the first instruction (device.md §6.7).
    let filter = LogFilter {
        pid: Some(p.pid),
        sources: vec![LogSourceKind::OsLog, LogSourceKind::Stdout],
        ..Default::default()
    };
    let mut logs = h.handle.logs(filter).await.expect("logs");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(p.exit.exited().is_none(), "still suspended");
    h.handle.signal(p.pid, 19).await.expect("SIGCONT");
    let until = Instant::now() + Duration::from_secs(4);
    let (mut oslog, mut stdout) = (0, 0);
    while Instant::now() < until {
        if let Ok(Some(b)) = tokio::time::timeout_at(until.into(), logs.next_batch()).await {
            for e in b.entries {
                if e.source == LogSource::Stdout { stdout += 1 } else { oslog += 1 }
            }
        }
    }
    eprintln!("after SIGCONT: {oslog} os_trace entries, {stdout} console lines");
    assert!(stdout > 0, "a console line arrived");
    assert!(oslog > 0, "os_trace entries from the first instruction");
    // Kill it: the exit is seen within 2 s.
    let mut exit = p.exit.clone();
    let t = Instant::now();
    h.handle.signal(p.pid, 9).await.expect("SIGKILL");
    let e = tokio::time::timeout(Duration::from_secs(2), exit.wait()).await.expect("app.exited within 2 s");
    eprintln!("exit seen by {} after {:?}", e.detected_by, t.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane A: needs the iPhone (RUSSET_LANE=A, RUSSET_HIL_UDID); run with --ignored"]
async fn hil_a_dead_tunnel_is_rebuilt_on_the_next_call() {
    let _serial = SERIAL.lock().await;
    let Some(h) = hil().await else { return };
    let udid = serde_json::json!({ "udid": h.handle.udid().as_str() });
    let before = h.conn.call(FacadeCall::new(facade::RSD_SERVICES, udid.clone())).await.expect("tunnel");
    let generation = before["generation"].as_u64().expect("generation");
    // Simulate the device side going away: the TCP stack stops, the tunnel stays in its slot.
    let killed = h.conn.call(FacadeCall::new(facade::TUNNEL_KILL, udid.clone())).await.expect("kill");
    assert_eq!(killed["killed"], true);
    let t = Instant::now();
    let stream = h.handle.connect_rsd("com.apple.coredevice.appservice").await.expect("rebuilt on the next call");
    drop(stream);
    let status = h.conn.call(FacadeCall::new(facade::TUNNEL_STATUS, udid)).await.expect("status");
    let after = status["generation"].as_u64().expect("generation");
    eprintln!("tunnel rebuilt: generation {generation} -> {after} in {:?}", t.elapsed());
    assert!(after > generation);
}
