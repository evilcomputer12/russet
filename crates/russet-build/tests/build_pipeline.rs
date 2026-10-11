// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The whole build pipeline on the RussetProbe fixture with a scripted `swift`, a fake Mach-O tool and fixed SDK
//! facts: no Swift toolchain, SDK or Apple file needed, so it runs in CI (B02, B03 planning and invocation, B06
//! assembly, B07 packaging, the report and the event log).

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use plist::Value;
use russet_build::{
    BuildEnv, BuildRequest, Builder, BundleIdPolicy, CleanScope, LintSeverity, NullSink, PackageKind, SignKind,
};
use russet_core::error::codes;
use support::{FakeMachO, FixedSdk, RecordingSink, ScriptedSwift, fake_binary, fake_facts, tmp};
use tokio_util::sync::CancellationToken;

const DESCRIBE: &str = r#"{
  "name" : "RussetProbe",
  "platforms" : [ { "name" : "ios", "version" : "17.0" } ],
  "products" : [
    { "name" : "RussetProbe", "targets" : [ "RussetProbe" ], "type" : { "library" : [ "automatic" ] } },
    { "name" : "ProbeWidget", "targets" : [ "ProbeWidget" ], "type" : { "library" : [ "automatic" ] } }
  ],
  "targets" : [
    { "name" : "RussetProbe", "module_type" : "SwiftTarget" },
    { "name" : "ProbeWidget", "module_type" : "SwiftTarget" }
  ]
}"#;

const EPOCH_SECS: u64 = 978_307_200;

struct World {
    _dir: tempfile::TempDir,
    root: Utf8PathBuf,
    project: Utf8PathBuf,
    swift: Arc<ScriptedSwift>,
    macho: Arc<FakeMachO>,
    builder: Builder,
}

fn world() -> World {
    let (dir, root) = tmp();
    let project = support::copy_probe(&root);
    let host_rpath = format!("{project}/.russet/work/swiftpm/out/Products/Debug-iphoneos/PackageFrameworks");
    let binary = fake_binary(
        &[
            "/usr/lib/swift",
            "@loader_path",
            &host_rpath,
            "/usr/lib/swift/usr/lib/swift-6.2/iphoneos",
            "@executable_path/Frameworks",
        ],
        &["/usr/lib/libSystem.B.dylib", "/System/Library/Frameworks/SwiftUI.framework/SwiftUI"],
    );
    let swift = ScriptedSwift::new(DESCRIBE, "russetprobe", binary);
    let macho = Arc::new(FakeMachO::default());
    let env =
        BuildEnv::new(Arc::new(FixedSdk(fake_facts(&root))), root.join("data"), root.join("cache"), swift.clone())
            .with_macho(macho.clone())
            .with_epoch(UNIX_EPOCH + Duration::from_secs(EPOCH_SECS));
    World { _dir: dir, root, project, swift, macho, builder: Builder::new(env).unwrap() }
}

fn info_plist(app: &Utf8Path) -> plist::Dictionary {
    let bytes = std::fs::read(app.join("Info.plist")).unwrap();
    assert!(bytes.starts_with(b"bplist00"), "Info.plist is binary");
    russet_plist::from_bytes(&bytes).unwrap().0.into_dictionary().unwrap()
}

fn hash_tree(root: &Utf8Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(d) = stack.pop() {
        for e in d.read_dir_utf8().unwrap() {
            let e = e.unwrap();
            let rel = e.path().strip_prefix(root).unwrap().to_string();
            if e.file_type().unwrap().is_dir() {
                out.insert(format!("{rel}/"), Vec::new());
                stack.push(e.path().to_owned());
            } else {
                out.insert(rel, std::fs::read(e.path()).unwrap());
            }
        }
    }
    out
}

#[tokio::test]
async fn a_build_assembles_the_app_with_scene_manifest_dt_keys_and_hygiene() {
    let w = world();
    let sink = RecordingSink::default();
    let out = w.builder.build(BuildRequest::new(&w.project), &sink, CancellationToken::new()).await.unwrap();

    let app = w.project.join(".russet/build/debug-iphoneos/RussetProbe.app");
    assert_eq!(out.app.path, app);
    assert_eq!(out.app.executable, app.join("RussetProbe"));
    assert_eq!(out.app.bundle_id, "dev.russet.test.probe");
    assert_eq!(std::fs::read(app.join("PkgInfo")).unwrap(), b"APPL????");
    let info = info_plist(&app);
    let s = |k: &str| info.get(k).and_then(Value::as_string).map(str::to_owned);
    assert_eq!(s("CFBundleIdentifier").as_deref(), Some("dev.russet.test.probe"));
    assert_eq!(s("CFBundleExecutable").as_deref(), Some("RussetProbe"));
    assert_eq!(s("MinimumOSVersion").as_deref(), Some("17.0"));
    assert_eq!(s("CFBundleDisplayName").as_deref(), Some("RussetProbe"));
    assert_eq!(s("DTSDKName").as_deref(), Some("iphoneos27.0"));
    assert_eq!(s("DTSDKBuild").as_deref(), Some("24A430"));
    assert_eq!(s("DTPlatformBuild").as_deref(), Some("24A430"));
    assert_eq!(s("DTXcode"), None, "metadata = sdk writes no Xcode identity (ARCHITECTURE §16.4)");
    assert!(
        info.get("UIApplicationSceneManifest").and_then(Value::as_dictionary).is_some(),
        "the scene manifest is present"
    );
    assert_eq!(info.get("UIDeviceFamily"), Some(&Value::Array(vec![Value::from(1), Value::from(2)])));

    // rpath hygiene removed the two host paths and kept the device paths.
    let deleted: Vec<String> = w.macho.deleted.lock().unwrap().iter().map(|(_, r)| r.clone()).collect();
    assert_eq!(deleted.len(), 2, "{deleted:?}");
    assert!(deleted.iter().all(|r| r.contains("PackageFrameworks") || r.contains("swift-6.2")));
    let exe = std::fs::read_to_string(app.join("RussetProbe"))
        .unwrap_or_else(|_| String::from_utf8_lossy(&std::fs::read(app.join("RussetProbe")).unwrap()).into_owned());
    assert!(
        exe.contains("RPATH /usr/lib/swift\n")
            && exe.contains("RPATH @executable_path/Frameworks\n")
            && !exe.contains("PackageFrameworks")
    );
    assert_eq!(out.lints.iter().filter(|l| l.code == "L006" && l.fixed).count(), 2);
    assert!(out.lints.iter().all(|l| l.severity <= LintSeverity::Warning));

    // The signing plan: the app, debug-signed.
    assert_eq!(out.signing_plan.items.len(), 1);
    assert_eq!(out.signing_plan.items[0].path, app);
    assert_eq!(out.signing_plan.items[0].kind, SignKind::Bundle);
    assert!(out.signing_plan.items[0].get_task_allow);
    assert_eq!(out.bundle_ids.get("dev.russet.test.probe").map(String::as_str), Some("dev.russet.test.probe"));
    assert_eq!(out.built.uuids.len(), 1);

    // build-report.json, schema 1.
    assert_eq!(out.report_path, w.project.join(".russet/build/debug-iphoneos/build-report.json"));
    let report: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out.report_path).unwrap()).unwrap();
    assert_eq!(report["schema"], 1);
    assert_eq!(report["buildId"], out.build_id.as_str());
    assert_eq!(report["profile"], "debug");
    assert_eq!(report["project"]["backend"], "swiftbuild");
    assert_eq!(report["toolchain"]["sdk"]["id"], "xcode-27.0-27A266a");
    assert_eq!(report["metadata"]["policy"], "sdk");
    assert_eq!(report["metadata"]["stamped"]["DTSDKBuild"], "24A430");
    assert_eq!(report["bundles"][0]["machO"][0]["sdk"], "27.0");
    assert_eq!(report["bundles"][0]["machO"][0]["minos"], "17.0");
    assert_eq!(report["bundles"][0]["machO"][0]["builtByRusset"], true);
    assert_eq!(report["artifacts"]["app"], ".russet/build/debug-iphoneos/RussetProbe.app");
    assert_eq!(report["signingPlan"][0]["getTaskAllow"], true);

    // The event log mirrors the job's events.
    let log = w.project.join(".russet/logs").join(format!("{}.jsonl", out.build_id));
    let events: Vec<serde_json::Value> =
        std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let stages: Vec<String> = events
        .iter()
        .filter(|e| e["kind"] == "stage" && e["state"] == "finished")
        .map(|e| e["stage"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(stages, ["resolve", "prepare", "compile", "resources", "assemble", "lint"]);
    assert!(events.iter().any(|e| e["kind"] == "progress" && e["done"] == 1 && e["total"] == 2));
    assert!(events.iter().any(|e| e["kind"] == "artifact" && e["artifactKind"] == "app"));
    assert!(events.windows(2).all(|w| w[0]["seq"].as_u64() < w[1]["seq"].as_u64()));
    assert!(!sink.events.lock().unwrap().is_empty());
    assert!(sink.lines.lock().unwrap().iter().any(|(_, l)| l.contains("Build complete")));

    // swift build ran with the SDK's toolset, the platform view, and no SDKROOT.
    let calls = w.swift.calls.lock().unwrap().clone();
    let build = calls.iter().find(|c| c[0] == "build").unwrap();
    for needle in ["--build-system", "swiftbuild", "--triple", "arm64-apple-ios", "--disable-automatic-resolution"] {
        assert!(build.iter().any(|a| a == needle), "{needle} missing from {build:?}");
    }
    assert!(build.iter().any(|a| a.ends_with("toolset-swb.json")));
    let env = w
        .swift
        .envs
        .lock()
        .unwrap()
        .iter()
        .find(|e| e.contains_key(std::ffi::OsStr::new("XCODE_EXTRA_PLATFORM_FOLDERS")))
        .cloned()
        .unwrap();
    assert!(!env.contains_key(std::ffi::OsStr::new("SDKROOT")));
    let builder = std::fs::read_to_string(w.project.join(".russet/work/builder/Package.swift")).unwrap();
    assert!(builder.contains("\"RussetProbe-App\"") && builder.contains(".iOS(\"17.0\")"));

    // Every file and directory carries the fixed time.
    let mtime = |p: &Utf8Path| std::fs::metadata(p).unwrap().modified().unwrap();
    assert_eq!(mtime(&app), UNIX_EPOCH + Duration::from_secs(EPOCH_SECS));
    assert_eq!(mtime(&app.join("Info.plist")), UNIX_EPOCH + Duration::from_secs(EPOCH_SECS));
}

#[tokio::test]
async fn two_clean_builds_are_byte_identical() {
    let w = world();
    let mut r = BuildRequest::new(&w.project);
    r.clean = true;
    let a = w.builder.build(r.clone(), &NullSink, CancellationToken::new()).await.unwrap();
    let first = hash_tree(&a.app.path);
    let b = w.builder.build(r, &NullSink, CancellationToken::new()).await.unwrap();
    assert_eq!(first, hash_tree(&b.app.path));
    assert_ne!(a.build_id, b.build_id);
}

#[tokio::test]
async fn ipa_packaging_follows_the_request() {
    let w = world();
    let mut r = BuildRequest::new(&w.project);
    r.package = Some(PackageKind::IpaDev);
    let out = w.builder.build(r, &NullSink, CancellationToken::new()).await.unwrap();
    let ipa = out.ipa.clone().unwrap();
    assert_eq!(ipa, w.project.join(".russet/build/debug-iphoneos/RussetProbe.ipa"));
    let listing = russet_ipa::read_listing(&ipa).unwrap();
    let exe = listing.iter().find(|e| e.name == "Payload/RussetProbe.app/RussetProbe").unwrap();
    assert_eq!(exe.unix_mode, 0o100755);
    assert_eq!(listing.iter().find(|e| e.name == "Payload/RussetProbe.app/Info.plist").unwrap().unix_mode, 0o100644);
    let report: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out.report_path).unwrap()).unwrap();
    assert_eq!(report["artifacts"]["ipa"], ".russet/build/debug-iphoneos/RussetProbe.ipa");
}

#[tokio::test]
async fn compiler_errors_become_diagnostics_and_compile_failed() {
    let (dir, root) = tmp();
    let project = support::copy_probe(&root);
    let mut swift = ScriptedSwift::new(DESCRIBE, "russetprobe", fake_binary(&[], &[]));
    Arc::get_mut(&mut swift).unwrap().fail_with =
        Some(format!("{project}/Sources/RussetProbe/ContentView.swift:13:9: error: cannot find 'Foo' in scope"));
    let env =
        BuildEnv::new(Arc::new(FixedSdk(fake_facts(&root))), root.join("data"), root.join("cache"), swift.clone())
            .with_macho(Arc::new(FakeMachO::default()));
    let builder = Builder::new(env).unwrap();
    let err = builder.build(BuildRequest::new(&project), &NullSink, CancellationToken::new()).await.unwrap_err();
    assert_eq!(err.code(), codes::build::COMPILE_FAILED);
    assert!(err.message().contains("cannot find 'Foo' in scope"));
    let details = err.to_data().details.unwrap();
    assert_eq!(details["errors"][0]["line"], 13);
    assert_eq!(details["errors"][0]["file"], format!("{project}/Sources/RussetProbe/ContentView.swift"));
    assert!(!project.join(".russet/build/debug-iphoneos/RussetProbe.app").exists());
    drop(dir);
}

#[tokio::test]
async fn a_non_project_directory_is_project_not_found() {
    let w = world();
    let empty = w.root.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let err = w.builder.build(BuildRequest::new(&empty), &NullSink, CancellationToken::new()).await.unwrap_err();
    assert_eq!(err.code(), codes::config::PROJECT_NOT_FOUND);
    assert_eq!(err.code().exit_code(), 4);
}

#[tokio::test]
async fn a_typo_in_russet_toml_reports_file_and_line() {
    let w = world();
    let toml = w.project.join("russet.toml");
    let text = std::fs::read_to_string(&toml).unwrap().replace("display-name", "display-names");
    std::fs::write(&toml, text).unwrap();
    let err = w.builder.build(BuildRequest::new(&w.project), &NullSink, CancellationToken::new()).await.unwrap_err();
    assert_eq!(err.code(), codes::config::UNKNOWN_KEY);
    assert!(err.message().contains("russet.toml:14:"), "{}", err.message());
}

#[tokio::test]
async fn policies_rewrite_bundle_ids_and_xtool_prefix_needs_the_prefix() {
    let w = world();
    let mut r = BuildRequest::new(&w.project);
    r.bundle_id_policy = Some(BundleIdPolicy::XtoolPrefix { prefix: "ab12cd34".into() });
    let out = w.builder.build(r, &NullSink, CancellationToken::new()).await.unwrap();
    assert_eq!(out.app.bundle_id, "XTL-AB12CD34.dev.russet.test.probe");
    assert_eq!(out.app.original_bundle_id, "dev.russet.test.probe");
    assert_eq!(
        info_plist(&out.app.path).get("CFBundleIdentifier").and_then(Value::as_string),
        Some("XTL-AB12CD34.dev.russet.test.probe")
    );
    std::fs::write(w.project.join("russet.local.toml"), "[signing]\nbundle-id-policy = \"xtool-prefix\"\n").unwrap();
    let err = w.builder.build(BuildRequest::new(&w.project), &NullSink, CancellationToken::new()).await.unwrap_err();
    assert_eq!(err.code(), codes::build::BUNDLE_ID_INVALID);
    std::fs::write(w.project.join("russet.local.toml"), "[app]\nbundle-id = \"com.example.mine\"\n[signing]\nbundle-id-policy = \"team-suffix\"\nteam = \"TEAM123456\"\n").unwrap();
    let out = w.builder.build(BuildRequest::new(&w.project), &NullSink, CancellationToken::new()).await.unwrap();
    assert_eq!(out.app.bundle_id, "com.example.mine.TEAM123456");
}

#[tokio::test]
async fn a_running_build_holds_the_project_lock() {
    let w = world();
    let work = w.project.join(".russet/work");
    std::fs::create_dir_all(&work).unwrap();
    let held = std::fs::File::options().create(true).truncate(false).write(true).open(work.join("build.lock")).unwrap();
    held.try_lock().unwrap();
    let err = w.builder.build(BuildRequest::new(&w.project), &NullSink, CancellationToken::new()).await.unwrap_err();
    assert_eq!(err.code(), codes::build::IN_PROGRESS);
    held.unlock().unwrap();
    w.builder.build(BuildRequest::new(&w.project), &NullSink, CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn plan_is_a_dry_run() {
    let w = world();
    let plan = w.builder.plan(&BuildRequest::new(&w.project)).await.unwrap();
    assert_eq!(plan.product.as_deref(), Some("RussetProbe"));
    assert_eq!(plan.configuration, "debug");
    assert!(!plan.release);
    assert_eq!(plan.app_path, Some(w.project.join(".russet/build/debug-iphoneos/RussetProbe.app")));
    assert_eq!(plan.bundle_ids.get("dev.russet.test.probe").map(String::as_str), Some("dev.russet.test.probe"));
    assert!(!plan.stages.contains(&russet_build::Stage::Package));
    assert!(w.swift.calls.lock().unwrap().is_empty(), "planning runs no tool");
    assert!(!w.project.join(".russet").exists(), "planning writes nothing into the project");
}

#[tokio::test]
async fn the_info_plist_preview_carries_provenance() {
    let w = world();
    let (plist, provenance) = w.builder.info_plist_with_provenance(&BuildRequest::new(&w.project), None).await.unwrap();
    assert_eq!(plist.get("CFBundleIdentifier").and_then(Value::as_string), Some("dev.russet.test.probe"));
    assert_eq!(provenance.get("CFBundleIdentifier").map(String::as_str), Some("enforced"));
    assert_eq!(provenance.get("CFBundleShortVersionString").map(String::as_str), Some("russet.toml"));
    assert_eq!(provenance.get("DTSDKName").map(String::as_str), Some("sdk"));
    assert_eq!(provenance.get("UIApplicationSceneManifest").map(String::as_str), Some("default"));
}

#[tokio::test]
async fn clean_removes_build_outputs() {
    let w = world();
    w.builder.build(BuildRequest::new(&w.project), &NullSink, CancellationToken::new()).await.unwrap();
    let report = w.builder.clean(&w.project, CleanScope::Build).await.unwrap();
    assert!(report.freed_bytes > 0);
    assert!(!w.project.join(".russet/build").exists() && !w.project.join(".russet/work").exists());
    assert!(w.project.join(".russet/logs").exists());
    w.builder.clean(&w.project, CleanScope::All).await.unwrap();
    assert!(!w.project.join(".russet").exists());
}

#[tokio::test]
async fn inspect_reads_apps_and_ipas() {
    let w = world();
    let mut r = BuildRequest::new(&w.project);
    r.package = Some(PackageKind::IpaStore);
    let out = w.builder.build(r, &NullSink, CancellationToken::new()).await.unwrap();
    for path in [out.app.path.clone(), out.ipa.clone().unwrap()] {
        let report = w.builder.inspect(&path).unwrap();
        assert_eq!(
            report.info_plist.get("CFBundleIdentifier").and_then(Value::as_string),
            Some("dev.russet.test.probe")
        );
        assert_eq!(report.dt.get("DTSDKName").map(String::as_str), Some("iphoneos27.0"));
        assert_eq!(report.mach_o.len(), 1, "{path}");
        assert!(report.mach_o[0].path.ends_with("RussetProbe.app/RussetProbe"));
        assert!(report.lints.iter().all(|l| l.code != "L006"), "the build removed host rpaths");
    }
    assert_eq!(w.builder.inspect(&w.root.join("missing.app")).unwrap_err().code(), codes::core::NOT_FOUND);
}

#[tokio::test]
async fn extensions_are_embedded_and_signed_first() {
    let w = world();
    let toml = w.project.join("russet.toml");
    let mut text = std::fs::read_to_string(&toml).unwrap();
    text.push_str(
        "\n[[app.extensions]]\nproduct = \"ProbeWidget\"\nkind = \"widgetkit\"\nbundle-id-suffix = \"widget\"\n",
    );
    std::fs::write(&toml, text).unwrap();
    let out = w.builder.build(BuildRequest::new(&w.project), &NullSink, CancellationToken::new()).await.unwrap();
    assert_eq!(out.extensions.len(), 1);
    let appex = &out.extensions[0];
    assert_eq!(appex.path, out.app.path.join("PlugIns/ProbeWidget.appex"));
    assert_eq!(appex.bundle_id, "dev.russet.test.probe.widget");
    let info = info_plist(&appex.path);
    assert_eq!(info.get("CFBundlePackageType").and_then(Value::as_string), Some("XPC!"));
    assert_eq!(info.get("CFBundleShortVersionString").and_then(Value::as_string), Some("1.0.0"));
    let items: Vec<&Utf8Path> = out.signing_plan.items.iter().map(|i| i.path.as_path()).collect();
    assert_eq!(items, [appex.path.as_path(), out.app.path.as_path()]);
    let builder = std::fs::read_to_string(w.project.join(".russet/work/builder/Package.swift")).unwrap();
    assert!(builder.contains("ProbeWidget-Extension") && builder.contains("_NSExtensionMain"));
}

#[tokio::test]
async fn the_deployment_target_must_be_valid_for_the_sdk() {
    let w = world();
    let toml = w.project.join("russet.toml");
    let text =
        std::fs::read_to_string(&toml).unwrap().replace("deployment-target = \"17.0\"", "deployment-target = \"16.9\"");
    std::fs::write(&toml, text).unwrap();
    let err = w.builder.build(BuildRequest::new(&w.project), &NullSink, CancellationToken::new()).await.unwrap_err();
    assert_eq!(err.code(), codes::config::INVALID);
    assert!(err.hint().is_some_and(|h| h.contains("17.0")));
}
