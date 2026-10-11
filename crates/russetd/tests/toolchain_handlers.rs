// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `toolchain.*` and `sdk.list` handlers over fixture toolchains (fake tools written by the test, no downloads),
//! and the doctor's configuration, secrets and toolchain checks over the same fixtures.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russet_config::{ConfigService, EnvOverrides};
use russet_core::dirs::RussetDirs;
use russet_core::error::codes;
use russet_protocol::doctor as d;
use russet_protocol::toolchain as p;
use russet_secrets::memory::MemoryStore;
use russet_toolchain::Toolchains;
use russet_toolchain::host::HostEnv;
use russet_toolchain::pins::PinManifest;
use russetd::handlers::doctor::DoctorServices;
use russetd::handlers::{doctor as dh, sdk as sh, toolchain as th};

/// A temporary directory removed on drop (russetd has no tempfile dev-dependency).
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let p = std::env::temp_dir().join(format!("russet-w3-tc-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).expect("temp dir");
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// A fake Swift 6.4 toolchain with LLDB 21 (no libpython needed), and a fake xtool-style Xcode 27 SDK bundle.
fn fixture(root: &Path) -> HostEnv {
    let tc = root.join("swift");
    script(&tc.join("usr/bin/swift"), "echo 'Swift version 6.4 (swift-6.4-RELEASE)'");
    script(
        &tc.join("usr/bin/lldb"),
        "case \"$*\" in *'platform list'*) echo 'remote-ios: Remote iOS platform plug-in.';; *) echo 'lldb version 21.0.0';; esac",
    );
    script(&tc.join("usr/bin/lldb-dap"), "echo 'LLVM version 21.0.0'");
    let b = root.join("sdks/darwin.artifactbundle");
    let sdk_root = "Developer/Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS27.0.sdk";
    std::fs::create_dir_all(b.join(sdk_root)).expect("sdk");
    std::fs::create_dir_all(b.join("Xcode.app/Contents")).expect("xcode");
    let files = [
        ("info.json", r#"{"schemaVersion":"1.0","artifacts":{"darwin":{"type":"swiftSDK","version":"0.0.1","variants":[{"path":"."}]}}}"#.to_owned()),
        ("swift-sdk.json", format!(r#"{{"schemaVersion":"4.0","targetTriples":{{"arm64-apple-ios":{{"sdkRootPath":"{sdk_root}"}}}}}}"#)),
        (&format!("{sdk_root}/SDKSettings.json"), r#"{"Version":"27.0"}"#.to_owned()),
        ("Xcode.app/Contents/version.plist", "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleShortVersionString</key><string>27.0</string><key>ProductBuildVersion</key><string>27A266a</string></dict></plist>".to_owned()),
        ("darwin-sdk-version.txt", "epoch=3,darwinTools=1.1.0,oam=1.3.0".to_owned()),
        ("toolset.json", r#"{"rootPath":"toolset/bin","linker":{"path":"ld64.lld"}}"#.to_owned()),
    ];
    for (name, text) in files {
        std::fs::write(b.join(name), text).expect(name);
    }
    script(&b.join("toolset/bin/ld64.lld"), "echo 'LLD 21.1.6'");
    let sys = root.join("sysbin");
    script(&sys.join("ld64.lld"), "echo 'LLD 23.1.1'");
    HostEnv {
        path: vec![tc.join("usr/bin"), sys],
        home: Some(root.to_path_buf()),
        swift_sdk_dirs: vec![root.join("sdks")],
        uv_python_dirs: vec![],
        swiftly_dirs: vec![],
        ldconfig: None,
    }
}

fn toolchains(home: &Path, host: HostEnv) -> Toolchains {
    let pins = PinManifest::embedded().expect("embedded pins");
    Toolchains::new(RussetDirs::under(home), Arc::new(pins)).with_host(host)
}

#[tokio::test]
async fn list_resolve_and_sdk_list() {
    let tmp = TempDir::new();
    let tc = toolchains(&tmp.path().join("home"), fixture(tmp.path()));
    let list = th::list(&tc, None, p::ToolchainListParams::default()).await.expect("list");
    assert_eq!(list.pins.source, "embedded");
    let linkers: Vec<_> = list.tools.iter().filter(|t| t.component == p::ToolComponent::Linker).collect();
    assert_eq!(linkers.len(), 2);
    assert!(linkers[0].ok && matches!(linkers[0].provenance, p::ToolProvenance::XtoolBundle { .. }));
    assert!(!linkers[1].ok && linkers[1].problems[0].contains("too old"));
    assert_eq!(list.sdks[0].id, "xcode-27.0-27A266a");
    let json = serde_json::to_value(&list).expect("json");
    assert_eq!(json["tools"][0]["component"], "swift");
    assert_eq!(json["tools"][0]["provenance"]["kind"], "system");

    let sdks = sh::list(&tc).await.expect("sdk.list");
    assert_eq!(sdks.sdks, list.sdks);

    let svc =
        ConfigService::with_env(&RussetDirs::under(&tmp.path().join("home")), EnvOverrides::default()).expect("config");
    let project = tmp.path().join("app");
    std::fs::create_dir_all(&project).expect("project");
    let params =
        |c: p::ToolComponent| p::ToolchainResolveParams { component: c, project: Some(project.display().to_string()) };
    let lldb = th::resolve(&tc, &svc, params(p::ToolComponent::Lldb)).await.expect("lldb");
    assert_eq!(lldb.version, "21.0.0");
    assert!(lldb.env.contains_key("LD_LIBRARY_PATH"));
    std::fs::write(project.join("russet.toml"), "[toolchain]\nswift = \"6.5\"\n").expect("pin");
    let err = th::resolve(&tc, &svc, params(p::ToolComponent::Swift)).await.expect_err("pinned 6.5");
    assert_eq!(err.code(), codes::toolchain::VERSION_MISMATCH);
    std::fs::write(project.join("russet.toml"), "[debug]\nlldb = \"llvm\"\n").expect("flavor");
    let err = th::resolve(&tc, &svc, params(p::ToolComponent::Lldb)).await.expect_err("no llvm lldb");
    assert_eq!(err.code(), codes::toolchain::MISSING);
    // A relative tool path in russet.toml is relative to the project, not to the daemon's directory.
    script(&project.join("tools/ld64.lld"), "echo 'LLD 23.1.4'");
    std::fs::write(project.join("russet.toml"), "[toolchain]\nlinker = \"tools/ld64.lld\"\n").expect("path");
    let linker = th::resolve(&tc, &svc, params(p::ToolComponent::Linker)).await.expect("project linker");
    assert_eq!(linker.path, project.join("tools/ld64.lld").display().to_string());
    assert_eq!((linker.version.as_str(), linker.provenance), ("23.1.4", p::ToolProvenance::ProjectPin));
    // A tool the project names that does not work is an error, never a "resolved" tool.
    script(&project.join("tools/ld64.lld"), "exit 3");
    let err = th::resolve(&tc, &svc, params(p::ToolComponent::Linker)).await.expect_err("broken project linker");
    assert_eq!(err.code(), codes::toolchain::BROKEN);
}

/// The first `ok` candidate of each component in `list`.
fn selected(list: &p::ToolchainListResult, c: p::ToolComponent) -> Option<&p::ToolInfo> {
    list.tools.iter().find(|t| t.component == c && t.ok)
}

#[tokio::test]
async fn list_with_a_project_shows_what_the_project_resolves_to() {
    let tmp = TempDir::new();
    let tc = toolchains(&tmp.path().join("home"), fixture(tmp.path()));
    let svc =
        ConfigService::with_env(&RussetDirs::under(&tmp.path().join("home")), EnvOverrides::default()).expect("config");
    let project = tmp.path().join("app");
    std::fs::create_dir_all(&project).expect("project");
    let with_project = || p::ToolchainListParams { project: Some(project.display().to_string()), refresh: None };

    // No pins: the same selection as without a project.
    let plain = th::list(&tc, None, p::ToolchainListParams::default()).await.expect("list");
    let list = th::list(&tc, Some(&svc), with_project()).await.expect("list");
    assert_eq!(list.tools, plain.tools);

    // A Swift pin the host lacks, the llvm LLDB flavor, and a linker named by path.
    script(&project.join("tools/ld64.lld"), "echo 'LLD 23.1.4'");
    std::fs::write(
        project.join("russet.toml"),
        "[toolchain]\nswift = \"6.5\"\nlinker = \"tools/ld64.lld\"\n[debug]\nlldb = \"llvm\"\n",
    )
    .expect("pins");
    let list = th::list(&tc, Some(&svc), with_project()).await.expect("list");
    assert_eq!(selected(&list, p::ToolComponent::Swift), None, "{:?}", list.tools);
    let swift = list.tools.iter().find(|t| t.component == p::ToolComponent::Swift).expect("swift");
    assert!(swift.problems.iter().any(|p| p.contains("requires swift 6.5")), "{:?}", swift.problems);
    let linker = selected(&list, p::ToolComponent::Linker).expect("the project's linker");
    assert_eq!((linker.provenance.clone(), linker.version.as_str()), (p::ToolProvenance::ProjectPin, "23.1.4"));
    let linkers: Vec<_> = list.tools.iter().filter(|t| t.component == p::ToolComponent::Linker).collect();
    assert_eq!(linkers.len(), 3, "the named linker comes first, then the probed ones");
    assert!(linkers[1].problems.iter().any(|p| p.contains("the project names")), "{:?}", linkers[1]);
    assert_eq!(selected(&list, p::ToolComponent::Lldb), None, "the fixture has no llvm LLDB");
    let lldb = list.tools.iter().find(|t| t.component == p::ToolComponent::Lldb).expect("swift lldb");
    assert!(lldb.problems.iter().any(|p| p.contains("asks for the llvm lldb")), "{:?}", lldb.problems);

    // The project's choices match what toolchain.resolve picks.
    std::fs::write(project.join("russet.toml"), "[toolchain]\nswift = \"6.4\"\n").expect("pins");
    let list = th::list(&tc, Some(&svc), with_project()).await.expect("list");
    let resolve =
        |c: p::ToolComponent| p::ToolchainResolveParams { component: c, project: Some(project.display().to_string()) };
    for c in [p::ToolComponent::Swift, p::ToolComponent::Linker, p::ToolComponent::Lldb] {
        let resolved = th::resolve(&tc, &svc, resolve(c)).await.expect("resolves");
        assert_eq!(selected(&list, c), Some(&resolved), "{c:?}");
    }
}

#[tokio::test]
async fn uninstall_and_gc_of_nothing() {
    let tmp = TempDir::new();
    let tc = toolchains(&tmp.path().join("home"), fixture(tmp.path()));
    let err = th::uninstall(
        &tc,
        p::ToolchainUninstallParams { component: p::ManagedComponent::PythonShared, version: "3.12.15".into() },
    )
    .await
    .expect_err("not installed");
    assert_eq!(err.code(), codes::core::NOT_FOUND);
    let gc = th::gc(&tc, p::ToolchainGcParams { dry_run: Some(true) }).await.expect("gc");
    assert_eq!((gc.freed_bytes, gc.removed.len()), (0, 0));
    let changed = th::changed_params(&[russet_toolchain::Component::LibPython]);
    assert_eq!(serde_json::to_value(&changed).expect("json"), serde_json::json!({"components": ["libpython"]}));
}

fn status_of<'a>(r: &'a d::DoctorReport, id: &str) -> &'a d::DoctorCheck {
    r.checks.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no check {id}"))
}

#[tokio::test]
async fn doctor_passes_toolchain_and_secrets_and_skips_usbmux_in_lane_d() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    let dirs = RussetDirs::under(&home);
    let services = DoctorServices {
        dirs: dirs.clone(),
        config: Ok(Arc::new(ConfigService::with_env(&dirs, EnvOverrides::default()).expect("config"))),
        secrets: Ok(Arc::new(MemoryStore::new())),
        toolchains: Ok(Arc::new(toolchains(&home, fixture(tmp.path())))),
        device_backend: "none".into(),
        live_refused: false,
        usbmux_address: Some(format!("unix:{}", tmp.path().join("no-mux").display())),
    };
    let mut registry = russet_doctor::Registry::new();
    for c in russet_doctor::checks().into_iter().chain(dh::checks()) {
        registry.register(c);
    }
    let report = dh::run(&registry, &services, d::DoctorRunParams::default()).await.expect("doctor");
    for id in [
        "toolchain.swift",
        "toolchain.sdk",
        "toolchain.linker",
        "toolchain.lldb",
        "toolchain.lldb-dap",
        "secrets.backend",
        "config.global",
    ] {
        let c = status_of(&report, id);
        assert_eq!(c.status, d::DoctorStatus::Pass, "{id}: {c:?}");
    }
    let linker = status_of(&report, "toolchain.linker");
    assert!(
        linker.message.contains("darwin-tools 1.1.0 ld64.lld (LLD 21.1.6)") && linker.message.contains("too old"),
        "{}",
        linker.message
    );
    let mux = status_of(&report, "usbmux.available");
    assert_eq!(mux.status, d::DoctorStatus::Skip);
    assert!(mux.message.contains("device backend is none"), "{}", mux.message);
    assert_eq!(status_of(&report, "config.project").status, d::DoctorStatus::Skip, "no project given");
    assert_eq!(status_of(&report, "toolchain.xtool").status, d::DoctorStatus::Skip, "xtool is optional");
    assert_eq!(report.summary.fail, 0, "{report:#?}");

    // An invalid global file fails config.global with its position; a filter narrows the run.
    std::fs::create_dir_all(&dirs.config).expect("config dir");
    std::fs::write(dirs.config.join("config.toml"), "[daemon]\nmax-build-jobs = \"many\"\n").expect("bad");
    let only = d::DoctorRunParams { checks: Some(vec!["config.".into()]), ..Default::default() };
    let report = dh::run(&registry, &services, only).await.expect("doctor");
    assert_eq!(report.checks.len(), 2);
    let g = status_of(&report, "config.global");
    assert_eq!(g.status, d::DoctorStatus::Fail);
    assert!(g.message.contains("config.toml:2:18"), "{}", g.message);
    assert!(g.hint.is_some());
}

fn services_for(home: &Path, host: HostEnv) -> DoctorServices {
    let dirs = RussetDirs::under(home);
    DoctorServices {
        dirs: dirs.clone(),
        config: Ok(Arc::new(ConfigService::with_env(&dirs, EnvOverrides::default()).expect("config"))),
        secrets: Ok(Arc::new(MemoryStore::new())),
        // Offline: a fix that tried to download would fail instead of reaching the network.
        toolchains: Ok(Arc::new(toolchains(home, host).with_offline(true))),
        device_backend: "scripted:/tmp/s.toml".into(),
        live_refused: false,
        usbmux_address: None,
    }
}

fn w3_registry() -> russet_doctor::Registry {
    let mut registry = russet_doctor::Registry::new();
    for c in dh::checks() {
        registry.register(c);
    }
    registry
}

#[tokio::test]
async fn an_lldb_without_remote_ios_is_the_users_to_fix() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    let host = fixture(tmp.path());
    // Swift's LLDB without the remote-ios platform: installing python-shared would not help.
    script(&tmp.path().join("swift/usr/bin/lldb"), "echo 'lldb version 21.0.0'");
    let services = services_for(&home, host);
    let registry = w3_registry();
    let only = |fix: bool| d::DoctorRunParams {
        checks: Some(vec!["toolchain.lldb".into()]),
        fix: fix.then_some(true),
        ..Default::default()
    };
    let report = dh::run(&registry, &services, only(false)).await.expect("doctor");
    let c = status_of(&report, "toolchain.lldb");
    assert_eq!(c.status, d::DoctorStatus::Fail, "{c:?}");
    assert!(c.message.contains("remote-ios"), "{}", c.message);
    assert_eq!(c.fix, Some(d::DoctorFix::Manual));
    let report = dh::run(&registry, &services, only(true)).await.expect("doctor --fix");
    let c = status_of(&report, "toolchain.lldb");
    assert_eq!((c.status, c.fix.clone()), (d::DoctorStatus::Fail, Some(d::DoctorFix::Manual)), "nothing installed");
    assert!(c.detail.is_none(), "no fix was attempted: {:?}", c.detail);
}

#[tokio::test]
async fn a_missing_linker_is_fixed_by_darwin_tools_unless_the_project_wants_another() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    let mut host = fixture(tmp.path());
    // No SDK bundle (so no bundle linker), only the system LLD 23.1.1, which is too old.
    host.swift_sdk_dirs.clear();
    let services = services_for(&home, host);
    let registry = w3_registry();
    let params = |project: Option<&Path>| d::DoctorRunParams {
        checks: Some(vec!["toolchain.linker".into()]),
        project: project.map(|p| p.display().to_string()),
        ..Default::default()
    };
    let report = dh::run(&registry, &services, params(None)).await.expect("doctor");
    let c = status_of(&report, "toolchain.linker");
    assert_eq!(c.status, d::DoctorStatus::Fail, "{c:?}");
    assert_eq!(c.fix, Some(d::DoctorFix::Auto { applied: false }));
    assert!(c.hint.as_deref().is_some_and(|h| h.contains("darwin-tools")), "{:?}", c.hint);

    let project = tmp.path().join("app");
    std::fs::create_dir_all(&project).expect("project");
    std::fs::write(project.join("russet.toml"), "[toolchain]\nlinker = \"llvm@>=23.1.3\"\n").expect("pin");
    let report = dh::run(&registry, &services, params(Some(&project))).await.expect("doctor");
    let c = status_of(&report, "toolchain.linker");
    assert_eq!((c.status, c.fix.clone()), (d::DoctorStatus::Fail, Some(d::DoctorFix::Manual)));
}

#[tokio::test]
async fn toolchain_changes_are_forwarded_to_every_connection_until_shutdown() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    let tc = toolchains(&home, fixture(tmp.path()));
    // A leftover managed install nothing selects: GC removes it and reports the linker as changed.
    let old = RussetDirs::under(&home).data.join("toolchains/darwin-tools/0.9.0");
    std::fs::create_dir_all(&old).expect("old install");
    std::fs::write(old.join(".russet-install.json"), "{}").expect("marker");
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = sent.clone();
    let notify: russetd::handlers::config::Notify =
        Arc::new(move |m: &'static str, v| sink.lock().expect("sent").push((m.to_owned(), v)));
    let shutdown = tokio_util::sync::CancellationToken::new();
    let task = th::forward_changes(&tc, shutdown.clone(), notify);
    let gc = th::gc(&tc, p::ToolchainGcParams { dry_run: None }).await.expect("gc");
    assert_eq!(gc.removed.len(), 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while sent.lock().expect("sent").is_empty() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let got = sent.lock().expect("sent").clone();
    assert_eq!(got, vec![("toolchain.changed".to_owned(), serde_json::json!({"components": ["linker"]}))]);
    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), task).await.expect("stops").expect("no panic");
}
