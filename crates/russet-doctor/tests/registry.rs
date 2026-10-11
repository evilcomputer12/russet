// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The doctor framework (CORE-12): filters, fixes, summaries, exit codes, extensions, and the host checks,
//! including `usbmux.available` never contacting the mux unless the backend is `live` [D 83].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use russet_core::dirs::RussetDirs;
use russet_core::error::Result;
use russet_doctor::{Category, Check, CheckCx, CheckResult, Filter, Fix, Registry, Requires, Status, checks};

struct Flaky {
    fixed: AtomicBool,
}

#[async_trait]
impl Check for Flaky {
    fn id(&self) -> &'static str {
        "test.flaky"
    }
    fn title(&self) -> &'static str {
        "fixable"
    }
    fn category(&self) -> Category {
        Category::Config
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        if self.fixed.load(Ordering::SeqCst) { CheckResult::pass("fixed") } else { CheckResult::fail("broken") }
    }
    async fn fix(&self, _cx: &CheckCx) -> Result<()> {
        self.fixed.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn fix_kind(&self) -> Fix {
        Fix::Auto
    }
}

struct Online;

#[async_trait]
impl Check for Online {
    fn id(&self) -> &'static str {
        "network.test"
    }
    fn title(&self) -> &'static str {
        "needs the network"
    }
    fn category(&self) -> Category {
        Category::Network
    }
    fn requires(&self) -> Requires {
        Requires { network: true, ..Requires::default() }
    }
    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        CheckResult::warn("reachable but slow")
    }
}

fn registry() -> Registry {
    let mut r = Registry::new();
    r.register(Arc::new(Flaky { fixed: AtomicBool::new(false) }));
    r.register(Arc::new(Online));
    r
}

#[tokio::test]
async fn filters_fixes_summaries_and_exit_codes() {
    let r = registry();
    let cx = CheckCx::default();
    let report = r.run(&cx, &Filter::default()).await;
    assert_eq!(report.checks.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["test.flaky", "network.test"]);
    assert_eq!(report.checks[0].result.status, Status::Fail);
    assert_eq!(report.checks[0].fix, Some(Fix::Auto));
    assert_eq!(report.checks[1].result.status, Status::Skip, "network checks need --online");
    assert_eq!((report.summary().fail, report.summary().skip), (1, 1));
    assert_eq!(report.exit_code(false), 4);

    let online = CheckCx { online: true, ..CheckCx::default() };
    let only_network = Filter { categories: vec![Category::Network], ..Filter::default() };
    let report = r.run(&online, &only_network).await;
    assert_eq!(report.checks.len(), 1);
    assert_eq!(report.exit_code(false), 0, "warnings pass");
    assert_eq!(report.exit_code(true), 4, "--strict fails on warnings");

    let by_prefix = Filter { checks: vec!["test.".into()], ..Filter::default() };
    assert_eq!(r.run(&cx, &by_prefix).await.checks.len(), 1);
    let fix = Filter { checks: vec!["test.flaky".into()], fix: true, ..Filter::default() };
    let report = r.run(&cx, &fix).await;
    assert!(report.checks[0].fix_applied);
    assert_eq!(report.checks[0].result.status, Status::Pass);
    assert_eq!(report.checks[0].fix, None, "passing checks offer no fix");
}

#[test]
fn extensions_are_typed() {
    struct Service(u32);
    let mut cx = CheckCx::default();
    assert!(cx.extension::<Service>().is_none());
    cx.insert(Arc::new(Service(7)));
    assert_eq!(cx.extension::<Service>().map(|s| s.0), Some(7));
    assert!(cx.extension::<String>().is_none());
}

fn host_check(id: &str) -> Arc<dyn Check> {
    checks().into_iter().find(|c| c.id() == id).expect(id)
}

#[cfg(unix)]
#[tokio::test]
async fn usbmux_is_never_contacted_unless_the_backend_is_live() {
    let tmp = tempfile::tempdir().expect("tmp");
    let sock = tmp.path().join("usbmuxd");
    let listener = std::os::unix::net::UnixListener::bind(&sock).expect("tripwire socket");
    listener.set_nonblocking(true).expect("nonblocking");
    let check = host_check("usbmux.available");
    let addr = Some(format!("unix:{}", sock.display()));
    for backend in ["none", "scripted:/tmp/scenario.toml", "replay:/tmp/t.rdt"] {
        let cx = CheckCx { device_backend: Some(backend.into()), usbmux_address: addr.clone(), ..CheckCx::default() };
        let r = check.run(&cx).await;
        assert_eq!(r.status, Status::Skip, "{backend}: {r:?}");
        assert!(r.summary.contains("not contacted") && r.summary.contains("exists"), "{}", r.summary);
    }
    let refused = CheckCx {
        device_backend: Some("live".into()),
        live_refused: true,
        usbmux_address: addr.clone(),
        ..CheckCx::default()
    };
    assert_eq!(check.run(&refused).await.status, Status::Skip);
    assert!(listener.accept().is_err(), "nothing connected to the tripwire");

    let live = CheckCx { device_backend: Some("live".into()), usbmux_address: addr, ..CheckCx::default() };
    let r = check.run(&live).await;
    assert_eq!(r.status, Status::Pass, "{r:?}");
    assert!(listener.accept().is_ok(), "the live backend connects");
    let gone = CheckCx {
        device_backend: Some("live".into()),
        usbmux_address: Some(format!("unix:{}", tmp.path().join("missing").display())),
        ..CheckCx::default()
    };
    let r = check.run(&gone).await;
    assert_eq!(r.status, Status::Fail);
    assert!(r.hint.is_some_and(|h| h.contains("usbmuxd")));
}

#[cfg(unix)]
#[tokio::test]
async fn dirs_private_is_fixed_by_doctor_fix() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tmp");
    let dirs = RussetDirs::under(&tmp.path().join("home"));
    let cx = CheckCx { dirs: Some(dirs.clone()), ..CheckCx::default() };
    let check = host_check("dirs.private");
    assert_eq!(check.run(&cx).await.status, Status::Warn, "missing directories are a warning");
    let mut r = Registry::new();
    r.register(check.clone());
    let report = r.run(&cx, &Filter { fix: true, ..Filter::default() }).await;
    assert!(report.checks[0].fix_applied);
    assert_eq!(report.checks[0].result.status, Status::Pass);
    std::fs::set_permissions(&dirs.state, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let r = check.run(&cx).await;
    assert_eq!(r.status, Status::Fail);
    assert!(r.summary.contains("mode 755"), "{}", r.summary);
}

#[tokio::test]
async fn host_disk_compares_against_the_reserve() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dirs = RussetDirs::under(tmp.path());
    let check = host_check("host.disk");
    let small = CheckCx { dirs: Some(dirs.clone()), min_free_space: Some(1), ..CheckCx::default() };
    assert_eq!(check.run(&small).await.status, Status::Pass);
    let huge = CheckCx { dirs: Some(dirs), min_free_space: Some(u64::MAX / 8), ..CheckCx::default() };
    let r = check.run(&huge).await;
    assert_eq!(r.status, Status::Fail);
    assert!(r.hint.is_some());
}

#[tokio::test]
async fn every_host_check_runs_and_ids_are_unique() {
    let mut r = Registry::new();
    for c in checks() {
        r.register(c);
    }
    let ids = r.ids();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ids.len());
    let cx = CheckCx { device_backend: Some("none".into()), ..CheckCx::default() };
    let report = r.run(&cx, &Filter::default()).await;
    assert_eq!(report.checks.len(), ids.len());
    let os = report.checks.iter().find(|c| c.id == "host.os").expect("host.os");
    assert_eq!(os.result.status, Status::Pass, "{os:?}");
}
