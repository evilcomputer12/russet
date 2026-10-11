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
    let list = th::list(&tc, p::ToolchainListParams::default()).await.expect("list");
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
        config: Arc::new(ConfigService::with_env(&dirs, EnvOverrides::default()).expect("config")),
        secrets: Arc::new(MemoryStore::new()),
        toolchains: Arc::new(toolchains(&home, fixture(tmp.path()))),
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

#[tokio::test]
async fn an_lldb_without_remote_ios_fails_with_an_automatic_fix() {
    let tmp = TempDir::new();
    let home = tmp.path().join("home");
    let host = fixture(tmp.path());
    // Swift's LLDB without the remote-ios platform: the check fails and offers the automatic fix.
    script(&tmp.path().join("swift/usr/bin/lldb"), "echo 'lldb version 21.0.0'");
    let dirs = RussetDirs::under(&home);
    let services = DoctorServices {
        dirs: dirs.clone(),
        config: Arc::new(ConfigService::with_env(&dirs, EnvOverrides::default()).expect("config")),
        secrets: Arc::new(MemoryStore::new()),
        toolchains: Arc::new(toolchains(&home, host)),
        device_backend: "scripted:/tmp/s.toml".into(),
        live_refused: false,
        usbmux_address: None,
    };
    let mut registry = russet_doctor::Registry::new();
    for c in dh::checks() {
        registry.register(c);
    }
    let params = d::DoctorRunParams { checks: Some(vec!["toolchain.lldb".into()]), ..Default::default() };
    let report = dh::run(&registry, &services, params).await.expect("doctor");
    let c = status_of(&report, "toolchain.lldb");
    assert_eq!(c.status, d::DoctorStatus::Fail, "{c:?}");
    assert!(c.message.contains("remote-ios"), "{}", c.message);
    assert_eq!(c.fix, Some(d::DoctorFix::Auto { applied: false }));
}
