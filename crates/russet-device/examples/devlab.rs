// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `devlab`: drives russet-device's live backend directly, without a daemon, for lane-A checks (PLAN §0 rule 4)
//! until W2's daemon and CLI carry the same calls. It refuses without `RUSSET_LANE=A` and holds the host-wide
//! live-device lock while it runs (a live daemon in the meantime gets 3019). Output is redacted: UDIDs keep only
//! their chip prefix and device names are never printed (AGENTS.md §8).
//!
//! ```text
//! RUSSET_LANE=A cargo run -p russet-device --example devlab -- list
//! … -- info | readiness | ddi-status | apps | processes | rsd
//! … -- launch <bundle> [--suspended] [--no-console] [--dvt] [--wait-secs N] [--sigcont-after-ms N] [--kill-after-ms N]
//! … -- signal <pid> <sig> | install <app|ipa> | uninstall <bundle>
//! … -- forward (--service NAME | --tunnel-port N | --usbmux-port N) [--bind tcp:ADDR|unix:PATH] [--secs N]
//! … -- logs [--pid N] [--bundle B] [--secs N] | tail [--pid N] [--bundle B] [--lines N]
//! … -- bench-sessions N [--gap-ms N] | bench-afc MiB | ddi-mount <dir> [--force] | watch [--secs N]
//! … -- follow-relaunch <bundle> | cryptexd-installed [--shape]
//! ```

#![allow(clippy::print_stdout)]

// Shared cargo target slots: recompile after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::Arc;
use std::time::{Duration, Instant};

use russet_core::dirs::RussetDirs;
use russet_device::io::{FacadeCall, LiveConnector, ServiceConnector, facade};
use russet_device::model::redact_udid;
use russet_device::{DeviceConfig, DeviceManager, DeviceSelector, ForwardTarget, LaunchRequest, LaunchVia};

fn redact(mut v: serde_json::Value) -> serde_json::Value {
    fn walk(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    match k.as_str() {
                        "udid" => {
                            if let Some(s) = x.as_str() {
                                *x = serde_json::Value::String(redact_udid(s));
                            }
                        }
                        "name" | "DeviceName" => *x = serde_json::Value::String("<redacted>".into()),
                        _ => walk(x),
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

fn show(label: &str, v: serde_json::Value) {
    println!("{label}: {}", serde_json::to_string_pretty(&redact(v)).unwrap_or_default());
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn num<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    flag(args, name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

struct Progress;
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
fn stamp() -> String {
    format!("{:>8.3}s", START.get_or_init(Instant::now).elapsed().as_secs_f64())
}
impl russet_core::progress::ProgressSink for Progress {
    fn begin(&self, title: &str, _c: bool) {
        eprintln!("[{}] {title}", stamp());
    }
    fn report(&self, msg: Option<&str>, pct: Option<u8>) {
        eprintln!("[{}] {} {}", stamp(), msg.unwrap_or(""), pct.map(|p| format!("{p}%")).unwrap_or_default());
    }
    fn end(&self, msg: Option<&str>) {
        eprintln!("[progress] done {}", msg.unwrap_or(""));
    }
    fn log(&self, _s: russet_core::progress::LogStream, line: &str) {
        eprintln!("[log] {line}");
    }
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {} ({}): {}", e.code().as_str(), e.code().number(), e.message());
        if let Some(h) = e.hint() {
            eprintln!("hint: {h}");
        }
        std::process::exit(i32::from(e.code().exit_code()));
    }
}

async fn run() -> russet_core::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().cloned().unwrap_or_else(|| "list".into());
    if std::env::var("RUSSET_LANE").ok().as_deref() != Some("A") {
        return Err(russet_core::Error::new(
            russet_core::error::codes::device::LIVE_BACKEND_REFUSED,
            "devlab talks to the real iPhone: lane A only (RUSSET_LANE=A)",
        ));
    }
    let home = std::env::var_os("RUSSET_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("russet-devlab"));
    // Never next to a live daemon (ARCHITECTURE §3.3 item 7): the host-wide live-device lock, held until exit.
    let _lock = russet_device::live_lock::take(&home)?;
    let dirs = RussetDirs::under(&home);
    let mut cfg = DeviceConfig::load(&dirs)?;
    if let Some(d) = std::env::var("DEVLAB_KEEP_WARM_MS").ok().and_then(|v| v.parse().ok()) {
        cfg.keep_warm = Duration::from_millis(d);
    }
    let t0 = Instant::now();
    let conn = Arc::new(LiveConnector::new(&cfg)?);
    let mgr = DeviceManager::start(cfg, conn.clone()).await?;
    let sel = DeviceSelector::parse(flag(&args, "--device").as_deref());
    match cmd.as_str() {
        "list" => {
            let devices = russet_device::rpc::device_list(&mgr).await?;
            eprintln!("device.list in {:?}", t0.elapsed());
            show("devices", serde_json::to_value(devices).unwrap_or_default());
            let t1 = Instant::now();
            let again = russet_device::rpc::device_list(&mgr).await?;
            eprintln!("device.list (warm) in {:?}, {} device(s)", t1.elapsed(), again.devices.len());
        }
        "info" => {
            let h = mgr.get(&sel)?;
            show("info", serde_json::to_value(russet_device::wire::info(&h.info().await?)).unwrap_or_default());
        }
        "readiness" => {
            let h = mgr.get(&sel)?;
            let t = Instant::now();
            let r = h.readiness(true).await?;
            eprintln!("readiness in {:?}", t.elapsed());
            show("readiness", serde_json::to_value(russet_device::wire::readiness(&r)).unwrap_or_default());
        }
        "ddi-status" => {
            let h = mgr.get(&sel)?;
            let t = Instant::now();
            let s = h.ddi_status().await?;
            eprintln!("ddi status in {:?}", t.elapsed());
            show("ddi", serde_json::to_value(russet_device::wire::ddi_status(&s)).unwrap_or_default());
        }
        "prepare" => {
            // device.prepare on a ready phone must be a no-op (DEV-13 acceptance: < 1 s). Refuses to run when no DDI is
            // mounted, so it can never start a mount by surprise.
            let h = mgr.get(&sel)?;
            if !h.ddi_status().await?.mounted {
                return Err(russet_device::error::invalid_argument(
                    "no DDI is mounted: prepare would mount one (that is DC-28's job)",
                ));
            }
            let t = Instant::now();
            let r = h.prepare(Default::default()).await?;
            eprintln!("device.prepare in {:?}", t.elapsed());
            show("readiness", serde_json::to_value(russet_device::wire::readiness(&r)).unwrap_or_default());
        }
        "ddi-check" => {
            // The dry run of a DDI mount: payloads plus a TSS ticket for this device; nothing is installed.
            let h = mgr.get(&sel)?;
            let dir = args.get(1).cloned().ok_or_else(|| russet_device::error::invalid_argument("ddi-check <dir>"))?;
            let t = Instant::now();
            let c = h.ddi_check(std::path::Path::new(&dir)).await?;
            println!(
                "DDI {} personalized for this device in {:?}: {}-byte ticket, image SHA-384 {}…",
                c.ddi_build.unwrap_or_default(),
                t.elapsed(),
                c.ticket_bytes,
                &c.image_sha384[..16]
            );
        }
        "watch" => {
            // Device events with timestamps (DEV-02: restart usbmuxd meanwhile; the device must be re-listed within 5 s).
            let secs = num(&args, "--secs", 60u64);
            let mut rx = mgr.events();
            let until = tokio::time::Instant::now() + Duration::from_secs(secs);
            println!("[{}] watching {secs} s; {} device(s) now", stamp(), mgr.devices().len());
            loop {
                let ev = tokio::select! {
                    _ = tokio::time::sleep_until(until) => break,
                    ev = rx.recv() => ev,
                };
                match ev {
                    Ok(russet_device::DeviceEvent::Added(s)) => println!("[{}] added {}", stamp(), s.udid.redacted()),
                    Ok(russet_device::DeviceEvent::Removed(u)) => println!("[{}] removed {}", stamp(), u.redacted()),
                    Ok(russet_device::DeviceEvent::Changed(_)) => {}
                    Err(_) => {}
                }
            }
        }
        "follow-relaunch" => {
            // device.md §6.7: a bundle's log stream follows a relaunch. Subscribes by bundle id, then launches the app
            // with a console, kills it, launches it again and kills it; counts the entries per launch and source.
            let h = mgr.get(&sel)?;
            let bundle = args
                .get(1)
                .cloned()
                .ok_or_else(|| russet_device::error::invalid_argument("follow-relaunch <bundle>"))?;
            let filter = russet_device::LogFilter {
                bundle_id: Some(bundle.clone()),
                sources: vec![russet_device::logs::LogSourceKind::OsLog, russet_device::logs::LogSourceKind::Stdout],
                ..Default::default()
            };
            let mut s = h.logs(filter).await?;
            let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
            let collector = tokio::spawn(async move {
                let mut counts: std::collections::BTreeMap<(u32, String), usize> = Default::default();
                loop {
                    let b = tokio::select! {
                        _ = &mut stop_rx => break,
                        b = s.next_batch() => b,
                    };
                    let Some(b) = b else { break };
                    for e in b.entries {
                        *counts.entry((e.pid.unwrap_or(0), format!("{:?}", e.source))).or_default() += 1;
                    }
                }
                counts
            });
            let env: std::collections::BTreeMap<String, String> =
                [("DYLD_PRINT_LIBRARIES".to_owned(), "1".to_owned())].into();
            let mut pids = Vec::new();
            for _ in 0..2 {
                let p = h
                    .launch(LaunchRequest {
                        bundle_id: bundle.clone(),
                        console: true,
                        kill_existing: true,
                        env: env.clone(),
                        ..Default::default()
                    })
                    .await?;
                pids.push(p.pid);
                tokio::time::sleep(Duration::from_secs(2)).await;
                h.signal(p.pid, 9).await?;
                tokio::time::sleep(Duration::from_secs(1)).await;
                drop(p);
            }
            let _ = stop_tx.send(());
            let counts = collector.await.unwrap_or_default();
            for (i, pid) in pids.iter().enumerate() {
                let of = |src: &str| counts.get(&(*pid, src.to_owned())).copied().unwrap_or(0);
                println!("launch {} (pid {pid}): os_trace {}, console {}", i + 1, of("Oslog"), of("Stdout"));
            }
            let others: usize = counts.iter().filter(|((p, _), _)| !pids.contains(p)).map(|(_, n)| n).sum();
            println!("entries from other pids: {others}");
        }
        "cryptexd-installed" => {
            // The raw cryptexd copy-installed reply, for the recorded fixture in ddi.rs. `--shape` prints only the key
            // paths and value types (to review what the reply holds before printing values).
            let h = mgr.get(&sel)?;
            let raw = h.copy_installed_raw().await?;
            fn shape(v: &plist::Value, path: &str, out: &mut Vec<String>) {
                match v {
                    plist::Value::Dictionary(d) => {
                        for (k, x) in d {
                            shape(x, &format!("{path}/{k}"), out);
                        }
                    }
                    plist::Value::Array(a) => {
                        for (i, x) in a.iter().enumerate() {
                            shape(x, &format!("{path}[{i}]"), out);
                        }
                    }
                    plist::Value::String(_) => out.push(format!("{path}: string")),
                    plist::Value::Integer(_) => out.push(format!("{path}: integer")),
                    plist::Value::Boolean(_) => out.push(format!("{path}: bool")),
                    plist::Value::Data(d) => out.push(format!("{path}: data[{}]", d.len())),
                    plist::Value::Date(_) => out.push(format!("{path}: date")),
                    plist::Value::Real(_) => out.push(format!("{path}: real")),
                    _ => out.push(format!("{path}: other")),
                }
            }
            match raw {
                None => println!("(no argv)"),
                Some(v) if has(&args, "--shape") => {
                    let mut out = Vec::new();
                    shape(&v, "", &mut out);
                    out.iter().for_each(|l| println!("{l}"));
                }
                Some(v) => {
                    let mut xml = Vec::new();
                    plist::to_writer_xml(&mut xml, &v)
                        .map_err(|e| russet_device::error::invalid_argument(format!("{e}")))?;
                    println!("{}", String::from_utf8_lossy(&xml));
                }
            }
            let parsed = russet_device::ddi::parse_installed(h.copy_installed_raw().await?.as_ref());
            eprintln!("parsed: {parsed:?}");
        }
        "ddi-mount" => {
            let h = mgr.get(&sel)?;
            let dir = args.get(1).cloned().ok_or_else(|| russet_device::error::invalid_argument("ddi-mount <dir>"))?;
            let s =
                h.ddi_mount(&russet_device::ddi::DdiSource::Dir(dir.into()), has(&args, "--force"), &Progress).await?;
            show("ddi", serde_json::to_value(russet_device::wire::ddi_status(&s)).unwrap_or_default());
        }
        "rsd" => {
            let h = mgr.get(&sel)?;
            let t = Instant::now();
            let v = conn
                .call(FacadeCall::new(facade::RSD_SERVICES, serde_json::json!({"udid": h.udid().as_str()})))
                .await?;
            let n = v.get("services").and_then(|s| s.as_object()).map(|s| s.len()).unwrap_or(0);
            eprintln!("tunnel + RSD in {:?}: {n} services, generation {}", t.elapsed(), v["generation"]);
            let status = conn
                .call(FacadeCall::new(facade::TUNNEL_STATUS, serde_json::json!({"udid": h.udid().as_str()})))
                .await?;
            show("tunnel", status);
        }
        "idle-close" => {
            // device.keep-warm: an unused tunnel without live streams closes (run with DEVLAB_KEEP_WARM_MS=1000).
            let h = mgr.get(&sel)?;
            let udid = serde_json::json!({"udid": h.udid().as_str()});
            conn.call(FacadeCall::new(facade::RSD_SERVICES, udid.clone())).await?;
            let held = h.connect_rsd("com.apple.coredevice.appservice").await?;
            let wait = Duration::from_millis(num(&args, "--wait-ms", 3000u64));
            tokio::time::sleep(wait).await;
            let with_stream = conn.call(FacadeCall::new(facade::TUNNEL_STATUS, udid.clone())).await?;
            drop(held);
            tokio::time::sleep(wait).await;
            let without = conn.call(FacadeCall::new(facade::TUNNEL_STATUS, udid)).await?;
            println!("with a live stream: {}; after it closed: {}", with_stream["state"], without["state"]);
        }
        "apps" => {
            let h = mgr.get(&sel)?;
            let apps = h.apps(russet_device::AppFilter { all: has(&args, "--all"), bundle_id_prefix: None }).await?;
            for a in apps {
                println!("{}  {}  dev={}", a.bundle_id, a.version.unwrap_or_default(), a.developer_app);
            }
        }
        "processes" => {
            let h = mgr.get(&sel)?;
            let t = Instant::now();
            let ps = h.processes().await?;
            eprintln!("{} processes in {:?}", ps.len(), t.elapsed());
            for p in ps.iter().filter(|p| p.bundle_id.is_some()) {
                println!("{} {} {}", p.pid, p.name, p.bundle_id.clone().unwrap_or_default());
            }
        }
        "launch" => {
            let h = mgr.get(&sel)?;
            let bundle =
                args.get(1).cloned().ok_or_else(|| russet_device::error::invalid_argument("launch <bundle>"))?;
            let mut env = std::collections::BTreeMap::new();
            for kv in args.iter().filter_map(|a| a.strip_prefix("--env=")) {
                if let Some((k, v)) = kv.split_once('=') {
                    env.insert(k.to_owned(), v.to_owned());
                }
            }
            if has(&args, "--dt-mode") {
                env.insert("OS_ACTIVITY_DT_MODE".to_owned(), "YES".to_owned());
                env.insert("NSUnbufferedIO".to_owned(), "YES".to_owned());
            }
            let req = LaunchRequest {
                bundle_id: bundle,
                suspended: has(&args, "--suspended"),
                kill_existing: true,
                console: !has(&args, "--no-console"),
                via: if has(&args, "--dvt") { LaunchVia::Dvt } else { LaunchVia::Auto },
                env,
                ..Default::default()
            };
            let t = Instant::now();
            let mut p = h.launch(req).await?;
            println!(
                "launched pid={} via={:?} exe_set={} in {:?}",
                p.pid,
                p.via,
                !p.executable.is_empty(),
                t.elapsed()
            );
            let mut lines = p.output.as_ref().map(|o| o.subscribe().1);
            let sigcont: Option<u64> = flag(&args, "--sigcont-after-ms").and_then(|v| v.parse().ok());
            let kill: Option<u64> = flag(&args, "--kill-after-ms").and_then(|v| v.parse().ok());
            let close_channel: Option<u64> = flag(&args, "--close-channel-after-ms").and_then(|v| v.parse().ok());
            let wait = Duration::from_secs(num(&args, "--wait-secs", 5u64));
            let start = Instant::now();
            let (mut sent_cont, mut sent_kill, mut closed) = (false, false, false);
            let mut kill_at = None;
            let mut seen = 0u64;
            while start.elapsed() < wait {
                if let Some(ms) = sigcont
                    && !sent_cont
                    && start.elapsed() >= Duration::from_millis(ms)
                {
                    h.signal(p.pid, 19).await?;
                    println!("SIGCONT sent at {:?}", start.elapsed());
                    sent_cont = true;
                }
                if let Some(ms) = close_channel
                    && !closed
                    && start.elapsed() >= Duration::from_millis(ms)
                {
                    p.close_channel();
                    println!("launch channel closed at {:?}", start.elapsed());
                    closed = true;
                }
                if let Some(ms) = kill
                    && !sent_kill
                    && start.elapsed() >= Duration::from_millis(ms)
                {
                    h.signal(p.pid, 9).await?;
                    kill_at = Some(Instant::now());
                    println!("SIGKILL sent at {:?}", start.elapsed());
                    sent_kill = true;
                }
                if let Some(e) = p.exit.exited() {
                    println!("exited ({}) {:?} after the kill", e.detected_by, kill_at.map(|k| k.elapsed()));
                    break;
                }
                if let Some(rx) = lines.as_mut() {
                    while let Ok(l) = rx.try_recv() {
                        seen += 1;
                        if seen <= 5 {
                            println!("console: {}", l.message.chars().take(120).collect::<String>());
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            println!("console lines: {seen}; still running: {}", p.exit.exited().is_none());
        }
        "first-logs" => {
            // Launch suspended → subscribe to the pid → SIGCONT (device.md §6.7): logs from the first instruction.
            let h = mgr.get(&sel)?;
            let bundle =
                args.get(1).cloned().ok_or_else(|| russet_device::error::invalid_argument("first-logs <bundle>"))?;
            let mut env = std::collections::BTreeMap::new();
            env.insert(
                "OS_ACTIVITY_DT_MODE".to_owned(),
                std::env::var("DEVLAB_DT_MODE").unwrap_or_else(|_| "YES".to_owned()),
            );
            for kv in args.iter().filter_map(|a| a.strip_prefix("--env=")) {
                if let Some((k, v)) = kv.split_once('=') {
                    env.insert(k.to_owned(), v.to_owned());
                }
            }
            let req = LaunchRequest {
                bundle_id: bundle,
                suspended: true,
                kill_existing: true,
                console: true,
                env,
                ..Default::default()
            };
            let p = h.launch(req).await?;
            let filter = russet_device::LogFilter {
                pid: Some(p.pid),
                sources: vec![russet_device::logs::LogSourceKind::OsLog, russet_device::logs::LogSourceKind::Stdout],
                ..Default::default()
            };
            let mut s = h.logs(filter).await?;
            let t = Instant::now();
            h.signal(p.pid, 19).await?;
            let until = Instant::now() + Duration::from_secs(num(&args, "--secs", 4u64));
            let (mut oslog, mut stdout, mut first) = (0usize, 0usize, None);
            while Instant::now() < until {
                match tokio::time::timeout_at(until.into(), s.next_batch()).await {
                    Ok(Some(b)) => {
                        for e in b.entries {
                            first.get_or_insert(t.elapsed());
                            match e.source {
                                russet_protocol::common::LogSource::Stdout => stdout += 1,
                                _ => oslog += 1,
                            }
                        }
                    }
                    _ => break,
                }
            }
            println!("pid {}: {oslog} oslog and {stdout} stdout entries; first after {first:?} from SIGCONT", p.pid);
            h.signal(p.pid, 9).await?;
        }
        "stdio-shape" => {
            // Device check D1 (DC-04): what a CoreDevice startStopped launch's stdio socket carries.
            use tokio::io::AsyncReadExt;
            let h = mgr.get(&sel)?;
            let bundle =
                args.get(1).cloned().ok_or_else(|| russet_device::error::invalid_argument("stdio-shape <bundle>"))?;
            let mut stdio = h.connect_rsd(russet_device::coredevice::service::OPEN_STDIO_SOCKET).await?;
            let mut id = [0u8; 16];
            stdio
                .read_exact(&mut id)
                .await
                .map_err(|e| russet_device::error::invalid_argument(format!("uuid: {e}")))?;
            println!("stdio socket: first 16 bytes are the UUID (version nibble {:x})", id[6] >> 4);
            let mut cd = russet_device::coredevice::CoreDeviceClient::new(
                h.connect_rsd(russet_device::coredevice::service::APP_SERVICE).await?,
            )
            .await?;
            let input = russet_device::coredevice::LaunchInput {
                bundle_id: bundle,
                start_suspended: true,
                terminate_existing: true,
                stdio: Some(uuid::Uuid::from_bytes(id)),
                env: [("DYLD_PRINT_LIBRARIES".to_owned(), "1".to_owned())].into(),
                ..Default::default()
            };
            let out = cd
                .invoke(russet_device::coredevice::feature::LAUNCH_APPLICATION, input.to_xpc())
                .await
                .map_err(|e| e.into_error("launch"))?;
            let launched = russet_device::coredevice::parse_launch(&out)?;
            let mut buf = vec![0u8; 4096];
            let quiet = tokio::time::timeout(Duration::from_millis(700), stdio.read(&mut buf)).await.is_err();
            println!("while suspended: no stdio bytes for 700 ms = {quiet}");
            h.signal(launched.pid, 19).await?;
            let n = tokio::time::timeout(Duration::from_secs(3), stdio.read(&mut buf))
                .await
                .map_err(|_| russet_device::error::invalid_argument("no output"))?
                .map_err(|e| russet_device::error::invalid_argument(format!("read: {e}")))?;
            let chunk = &buf[..n];
            let crlf = chunk.windows(2).filter(|w| w == b"\r\n").count();
            let bare_lf =
                chunk.iter().enumerate().filter(|(i, b)| **b == b'\n' && (*i == 0 || chunk[*i - 1] != b'\r')).count();
            println!("first read after SIGCONT: {n} bytes, {crlf} CRLF line ends, {bare_lf} bare LF");
            h.signal(launched.pid, 9).await?;
            let t = Instant::now();
            loop {
                match tokio::time::timeout(Duration::from_secs(3), stdio.read(&mut buf)).await {
                    Ok(Ok(0)) => {
                        println!("stdio EOF {:?} after SIGKILL", t.elapsed());
                        break;
                    }
                    Ok(Ok(_)) => continue,
                    Ok(Err(e)) => {
                        println!("stdio error {:?} after SIGKILL: {:?}", t.elapsed(), e.kind());
                        break;
                    }
                    Err(_) => {
                        println!("stdio still open 3 s after SIGKILL");
                        break;
                    }
                }
            }
        }
        "debug-session" => {
            // The M1 device half without pymobiledevice3: a debugproxy forward plus a suspended CoreDevice launch,
            // both kept open (the launch channel too) until `--secs` pass or Ctrl-C; LLDB attaches meanwhile.
            let h = mgr.get(&sel)?;
            let bundle =
                args.get(1).cloned().ok_or_else(|| russet_device::error::invalid_argument("debug-session <bundle>"))?;
            let bind = flag(&args, "--bind").map(|b| russet_device::debug::parse_bind(&b)).transpose()?;
            let service = flag(&args, "--service").unwrap_or_else(|| "com.apple.internal.dt.remote.debugproxy".into());
            let fwd = russet_device::debug::open_forward_with(&h, ForwardTarget::RsdService(service), bind).await?;
            let req = LaunchRequest {
                bundle_id: bundle,
                suspended: true,
                kill_existing: true,
                console: true,
                ..Default::default()
            };
            let p = h.launch(req).await?;
            println!("url={}", fwd.local.lldb_url());
            println!("pid={}", p.pid);
            println!("exe={}", p.executable);
            let secs = num(&args, "--secs", 120u64);
            let mut exit = p.exit.clone();
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(secs)) => println!("session timed out"),
                _ = tokio::signal::ctrl_c() => println!("interrupted"),
                e = exit.wait() => println!("app exited ({})", e.detected_by),
            }
            drop(p);
            drop(fwd);
        }
        "signal" => {
            let h = mgr.get(&sel)?;
            let pid: u32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
            let sig: i32 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(19);
            h.signal(pid, sig).await?;
            println!("signal {sig} sent to {pid}");
        }
        "install" => {
            let h = mgr.get(&sel)?;
            let path = args.get(1).cloned().ok_or_else(|| russet_device::error::invalid_argument("install <path>"))?;
            let r = h.install(std::path::Path::new(&path), Default::default(), &Progress).await?;
            println!("installed {} in {:?} ({} bytes)", r.bundle_id, r.duration, r.bytes);
        }
        "uninstall" => {
            let h = mgr.get(&sel)?;
            let b = args.get(1).cloned().ok_or_else(|| russet_device::error::invalid_argument("uninstall <bundle>"))?;
            h.uninstall(&b).await?;
            println!("uninstalled {b}");
        }
        "forward" => {
            let h = mgr.get(&sel)?;
            let target = if let Some(s) = flag(&args, "--service") {
                ForwardTarget::RsdService(s)
            } else if let Some(p) = flag(&args, "--tunnel-port").and_then(|p| p.parse().ok()) {
                ForwardTarget::TunnelPort(p)
            } else if let Some(p) = flag(&args, "--usbmux-port").and_then(|p| p.parse().ok()) {
                ForwardTarget::UsbmuxPort(p)
            } else {
                ForwardTarget::RsdService("com.apple.internal.dt.remote.debugproxy".into())
            };
            let bind = flag(&args, "--bind").map(|b| russet_device::debug::parse_bind(&b)).transpose()?;
            let f = russet_device::debug::open_forward_with(&h, target, bind).await?;
            println!("forward {} -> {}", f.local.lldb_url(), f.id);
            let secs = num(&args, "--secs", 600u64);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(secs)) => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            for info in mgr.forwards() {
                println!("connections: {}", info.connections);
            }
        }
        "logs" => {
            let h = mgr.get(&sel)?;
            let filter = russet_device::LogFilter {
                pid: flag(&args, "--pid").and_then(|p| p.parse().ok()),
                bundle_id: flag(&args, "--bundle"),
                backlog: num(&args, "--backlog", 0usize),
                ..Default::default()
            };
            let mut s = h.logs(filter).await?;
            let until = Instant::now() + Duration::from_secs(num(&args, "--secs", 4u64));
            let (mut n, mut batches) = (0usize, 0usize);
            while Instant::now() < until {
                match tokio::time::timeout_at(until.into(), s.next_batch()).await {
                    Ok(Some(b)) => {
                        batches += 1;
                        n += b.entries.len();
                    }
                    _ => break,
                }
            }
            println!("{n} entries in {batches} batches");
        }
        "tail" => {
            let h = mgr.get(&sel)?;
            let q = russet_device::logs::TailQuery {
                pid: flag(&args, "--pid").and_then(|p| p.parse().ok()),
                bundle_id: flag(&args, "--bundle"),
                lines: flag(&args, "--lines").and_then(|p| p.parse().ok()),
                ..Default::default()
            };
            let r = russet_device::logs::tail(&h, q).await?;
            println!("{} entries, truncated={}, cursor={:?}", r.entries.len(), r.truncated, r.cursor);
        }
        "bench-sessions" => {
            let h = mgr.get(&sel)?;
            let n: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(100);
            let (mut ok, mut fail) = (0, 0);
            let mut times = Vec::new();
            let gap = Duration::from_millis(num(&args, "--gap-ms", 0u64));
            let mut retried = Vec::new();
            for i in 0..n {
                conn.call(FacadeCall::new(facade::TUNNEL_CLOSE, serde_json::json!({"udid": h.udid().as_str()})))
                    .await?;
                tokio::time::sleep(gap).await;
                let before = conn.tunnel_open_retries();
                let t = Instant::now();
                match conn
                    .call(FacadeCall::new(facade::RSD_SERVICES, serde_json::json!({"udid": h.udid().as_str()})))
                    .await
                {
                    Ok(_) => {
                        ok += 1;
                        times.push(t.elapsed());
                        let r = conn.tunnel_open_retries() - before;
                        if r > 0 {
                            retried.push(format!("#{i}:{r}x/{}ms", t.elapsed().as_millis()));
                        }
                    }
                    Err(e) => {
                        fail += 1;
                        eprintln!("session open failed: {}", e.message());
                    }
                }
            }
            times.sort();
            let p95 = times.get(times.len().saturating_mul(95) / 100).copied().unwrap_or_default();
            println!(
                "{ok} ok, {fail} failed; median {:?}, p95 {p95:?}; device refusals retried: {} in {} opens {:?}",
                times.get(times.len() / 2).copied().unwrap_or_default(),
                conn.tunnel_open_retries(),
                retried.len(),
                retried
            );
        }
        "bench-lockdown" => {
            let h = mgr.get(&sel)?;
            let n: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(100);
            let (mut ok, mut fail) = (0, 0);
            for _ in 0..n {
                let v = conn
                    .call(FacadeCall::new(
                        facade::LOCKDOWN_VALUES,
                        serde_json::json!({"udid": h.udid().as_str(), "keys": ["ProductVersion"], "session": true}),
                    ))
                    .await;
                match v {
                    Ok(_) => ok += 1,
                    Err(e) => {
                        fail += 1;
                        eprintln!("lockdown session failed: {}", e.message());
                    }
                }
            }
            println!("{ok} ok, {fail} failed");
        }
        "bench-afc" => {
            let h = mgr.get(&sel)?;
            let mib: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(32);
            bench_afc(&h, mib).await?;
        }
        other => {
            return Err(russet_device::error::invalid_argument(format!("unknown devlab command {other}")));
        }
    }
    mgr.shutdown();
    Ok(())
}

/// AFC through the tunnel (`com.apple.afc.shim.remote`): the MSS clamp's acceptance (device.md DEV-04).
async fn bench_afc(h: &russet_device::DeviceHandle, mib: usize) -> russet_core::Result<()> {
    use idevice::afc::AfcClient;
    use idevice::afc::opcode::AfcFopenMode;
    let stream = h.connect_rsd("com.apple.afc.shim.remote").await?;
    let mut dev = idevice::Idevice::new(stream, "russet");
    dev.rsd_checkin().await.map_err(|e| russet_device::error::from_idevice(e, "afc checkin"))?;
    let mut afc = AfcClient::new(dev);
    let data: Vec<u8> = (0..mib * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let path = format!("/russet-bench-{}.bin", std::process::id());
    let t = Instant::now();
    {
        let mut fd = afc
            .open(path.as_str(), AfcFopenMode::WrOnly)
            .await
            .map_err(|e| russet_device::error::from_idevice(e, "open"))?;
        fd.write_entire(&data).await.map_err(|e| russet_device::error::from_idevice(e, "write"))?;
        fd.close().await.map_err(|e| russet_device::error::from_idevice(e, "close"))?;
    }
    let up = t.elapsed();
    let t = Instant::now();
    let back = {
        let mut fd = afc
            .open(path.as_str(), AfcFopenMode::RdOnly)
            .await
            .map_err(|e| russet_device::error::from_idevice(e, "open"))?;
        let b = fd.read_entire().await.map_err(|e| russet_device::error::from_idevice(e, "read"))?;
        fd.close().await.map_err(|e| russet_device::error::from_idevice(e, "close"))?;
        b
    };
    let down = t.elapsed();
    let _ = afc.remove(path.as_str()).await;
    let rate = |d: Duration| mib as f64 / d.as_secs_f64();
    println!(
        "AFC through the tunnel, {mib} MiB: up {:.1} MiB/s ({up:?}), down {:.1} MiB/s ({down:?}), identical={}",
        rate(up),
        rate(down),
        back == data
    );
    Ok(())
}
