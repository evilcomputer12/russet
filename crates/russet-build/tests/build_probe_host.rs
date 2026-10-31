// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! PLAN §4.5 tests on this host: RussetProbe builds with Russet's SwiftPM backend (`xtool` hidden from `PATH`),
//! `llvm-otool -l` shows `sdk 27.0` and `minos 17.0` on the executable and on a package dylib, no host `LC_RPATH`
//! is left, the scene manifest is present, two clean builds are byte-identical, the IPA is deterministic with
//! modes 755/644, and a warm build is fast.
//!
//! Local and env-gated: set `RUSSET_TEST_SDK_BUNDLE` to a Darwin SDK bundle (for example
//! `~/.swiftpm/swift-sdks/darwin.artifactbundle`, read in place). Needs Swift 6.4 on `PATH` and LLVM ≥ 23's
//! `llvm-otool` (the independent check of every Mach-O). Rpath hygiene and Mach-O summaries go through the
//! production `RussetMachO` (W6's russet-macho) once its bodies exist, and through `llvm-otool` and
//! `llvm-install-name-tool` while they are F0 stubs; each test prints which one it used.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use camino::{Utf8Path, Utf8PathBuf};
use plist::Value;
use russet_build::{BuildEnv, BuildRequest, Builder, MachOTool, PackageKind, RussetMachO, SignKind};
use russet_sdk::XtoolBundleProvider;
use russet_sdk::host::HostToolchain;
use support::{LlvmMachO, LocalRunner, RecordingSink};
use tokio_util::sync::CancellationToken;

struct Host {
    bundle: Utf8PathBuf,
    macho: Arc<dyn MachOTool>,
    host: HostToolchain,
    otool: Utf8PathBuf,
}

async fn host() -> Option<Host> {
    let Some(bundle) = std::env::var_os("RUSSET_TEST_SDK_BUNDLE") else {
        eprintln!("skipped: set RUSSET_TEST_SDK_BUNDLE to a Darwin SDK bundle");
        return None;
    };
    let bundle = Utf8PathBuf::from_path_buf(bundle.into()).unwrap();
    let host = HostToolchain::probe(&LocalRunner).await.unwrap();
    let Some(otool) = host.llvm_otool.clone() else {
        eprintln!("skipped: needs llvm-otool from LLVM 23 or newer");
        return None;
    };
    let macho: Arc<dyn MachOTool> = if support::russet_macho_is_implemented() {
        eprintln!("Mach-O tool: russet-macho (RussetMachO, the production default)");
        Arc::new(RussetMachO)
    } else {
        let Some(llvm) = LlvmMachO::find().await else {
            eprintln!("skipped: russet-macho is still F0 and llvm-install-name-tool (LLVM >= 23) is missing");
            return None;
        };
        eprintln!("Mach-O tool: llvm-otool and llvm-install-name-tool (russet-macho is still the F0 stub)");
        Arc::new(llvm)
    };
    Some(Host { bundle, macho, host, otool })
}

fn builder(h: &Host, root: &Utf8Path) -> Builder {
    let data = root.join("data");
    let provider =
        XtoolBundleProvider::new(&data, h.host.clone()).with_search_dirs(vec![h.bundle.parent().unwrap().to_owned()]);
    let env = BuildEnv::new(Arc::new(provider), &data, root.join("cache"), Arc::new(LocalRunner))
        .with_macho(Arc::clone(&h.macho));
    Builder::new(env).unwrap()
}

fn otool(h: &Host, path: &Utf8Path) -> String {
    let out = std::process::Command::new(h.otool.as_std_path()).arg("-l").arg(path.as_str()).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The `LC_RPATH`s in `llvm-otool -l` output.
fn rpaths(otool_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cmd = "";
    for line in otool_text.lines().map(str::trim) {
        if let Some(c) = line.strip_prefix("cmd ") {
            cmd = c;
        } else if cmd == "LC_RPATH"
            && let Some(p) = line.strip_prefix("path ")
        {
            out.push(p.split(" (offset").next().unwrap_or(p).to_owned());
        }
    }
    out
}

fn hash_tree(root: &Utf8Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(d) = stack.pop() {
        for e in d.read_dir_utf8().unwrap() {
            let e = e.unwrap();
            let rel = e.path().strip_prefix(root).unwrap().to_string();
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path().to_owned());
                out.insert(format!("{rel}/"), Vec::new());
            } else {
                out.insert(rel, std::fs::read(e.path()).unwrap());
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn russet_probe_builds_on_this_host() {
    let Some(h) = host().await else { return };
    let (_dir, root) = support::tmp();
    let project = support::copy_probe(&root);
    let b = builder(&h, &root);
    let sink = RecordingSink::default();

    let t0 = Instant::now();
    let out = b.build(BuildRequest::new(&project), &sink, CancellationToken::new()).await.unwrap_or_else(|e| {
        let lines = sink.lines.lock().unwrap();
        panic!("build failed: {e:?}\n{}", lines.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n"))
    });
    eprintln!("cold build: {:.1} s", t0.elapsed().as_secs_f64());

    // sdk 27.0 / minos 17.0 on the executable (and on every Mach-O Russet built).
    let text = otool(&h, &out.app.executable);
    assert!(text.contains("platform 2"), "{text}");
    assert!(text.contains("sdk 27.0"), "LC_BUILD_VERSION sdk: {text}");
    assert!(text.contains("minos 17.0"), "LC_BUILD_VERSION minos: {text}");
    // No absolute host rpath is left.
    let rpaths = rpaths(&text);
    assert!(!rpaths.is_empty());
    for r in &rpaths {
        assert!(r.starts_with('@') || r == "/usr/lib/swift", "host rpath left: {r} in {rpaths:?}");
    }
    let fixed: Vec<&russet_build::LintFinding> = out.lints.iter().filter(|l| l.code == "L006").collect();
    assert!(!fixed.is_empty() && fixed.iter().all(|l| l.fixed));
    assert!(fixed.iter().all(|l| l.path.as_deref() == Some(Utf8Path::new("RussetProbe.app/RussetProbe"))));
    // The report lists the executable relative to its bundle (build §9.6) with the linked versions.
    let report: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out.report_path).unwrap()).unwrap();
    assert_eq!(report["bundles"][0]["machO"][0]["path"], "RussetProbe");
    assert_eq!(report["bundles"][0]["machO"][0]["sdk"], "27.0");
    assert_eq!(report["bundles"][0]["machO"][0]["minos"], "17.0");
    assert_eq!(report["bundles"][0]["machO"][0]["arch"], "arm64");
    for m in &out.lints {
        eprintln!("lint {} {:?} {} (fixed: {})", m.code, m.severity, m.message, m.fixed);
    }
    // The scene manifest and the sdk-policy DT keys.
    let info = russet_plist::dictionary_from_path(out.app.path.join("Info.plist")).unwrap();
    assert!(info.get("UIApplicationSceneManifest").and_then(Value::as_dictionary).is_some());
    assert_eq!(info.get("DTSDKName").and_then(Value::as_string), Some("iphoneos27.0"));
    assert!(info.get("DTPlatformBuild").and_then(Value::as_string).is_some_and(|v| !v.is_empty()));
    assert!(!info.contains_key("DTXcode"));
    assert_eq!(info.get("CFBundleIdentifier").and_then(Value::as_string), Some("dev.russet.test.probe"));
    assert!(out.report_path.is_file());
    assert_eq!(out.signing_plan.items.last().map(|i| i.path.clone()), Some(out.app.path.clone()));
    // A debug build signs the app with get-task-allow (the debugger attaches, PLAN §4.10).
    assert!(out.signing_plan.items.last().is_some_and(|i| i.get_task_allow));
    debug_map_reaches_the_breakpoint_source(&h, &out.app.executable);
    let first = hash_tree(&out.app.path);

    // A warm build (nothing changed) is fast (PLAN §4.5: ≤ 60 s warm).
    let t1 = Instant::now();
    let warm = b.build(BuildRequest::new(&project), &sink, CancellationToken::new()).await.unwrap();
    let warm_secs = t1.elapsed().as_secs_f64();
    eprintln!("warm build: {warm_secs:.1} s");
    assert!(warm_secs <= 60.0);
    assert_eq!(hash_tree(&warm.app.path), first);

    // Two clean builds give byte-identical bundles before signing.
    let mut clean = BuildRequest::new(&project);
    clean.clean = true;
    clean.package = Some(PackageKind::IpaDev);
    let again = b.build(clean, &sink, CancellationToken::new()).await.unwrap();
    assert_eq!(hash_tree(&again.app.path), first, "clean rebuilds are byte-identical");

    // The IPA: modes 755/644, and the same bytes twice.
    let ipa = again.ipa.clone().unwrap();
    let listing = russet_ipa::read_listing(&ipa).unwrap();
    let mode = |n: &str| listing.iter().find(|e| e.name == n).map(|e| e.unix_mode);
    assert_eq!(mode("Payload/RussetProbe.app/RussetProbe"), Some(0o100755));
    assert_eq!(mode("Payload/RussetProbe.app/Info.plist"), Some(0o100644));
    let sha1 = sha(&ipa);
    let other = root.join("again.ipa");
    let report =
        russet_ipa::write(&again.app.path, &other, russet_ipa::IpaOptions::new(russet_ipa::IpaMode::Dev)).unwrap();
    assert_eq!(report.sha256, sha1, "two IPA runs give the same SHA-256");
}

/// The debug executable has no dSYM (`[build] dsym = "release"`), so LLDB reads DWARF through its debug map: the
/// `N_OSO` entries must name object files that still exist after the build (the persistent `.russet/work/swiftpm`
/// scratch), and `ContentView.swift`'s object must carry the line table of the `RUSSET_PROBE_BREAK` breakpoint
/// (W9's `russet debug --break ContentView.swift:<line>`).
fn debug_map_reaches_the_breakpoint_source(h: &Host, executable: &Utf8Path) {
    let tool = |name: &str| h.otool.parent().unwrap().join(name);
    let (nm, dwarfdump) = (tool("llvm-nm"), tool("llvm-dwarfdump"));
    if !nm.is_file() || !dwarfdump.is_file() {
        eprintln!("debug map not checked: no llvm-nm or llvm-dwarfdump next to llvm-otool");
        return;
    }
    let out = std::process::Command::new(nm.as_std_path()).arg("-a").arg(executable.as_str()).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let objects: Vec<&str> =
        text.lines().filter(|l| l.contains(" OSO ")).filter_map(|l| l.split(" OSO ").nth(1)).collect();
    assert!(!objects.is_empty(), "no debug map in the debug executable");
    for o in &objects {
        let file = o.split('(').next().unwrap_or(o).trim();
        assert!(Utf8Path::new(file).exists(), "debug-map object {file} is gone after the build");
    }
    let content_view =
        objects.iter().find(|o| o.contains("ContentView")).expect("ContentView's object in the debug map");
    let lines = std::process::Command::new(dwarfdump.as_std_path())
        .arg("--debug-line")
        .arg(content_view.split('(').next().unwrap().trim())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&lines.stdout).contains("ContentView.swift"),
        "no line table for ContentView.swift"
    );
}

fn sha(path: &Utf8Path) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(std::fs::read(path).unwrap()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The release profile: Swift Build's dSYM (or dsymutil), `llvm-strip -x`, L002 at error severity.
#[tokio::test(flavor = "multi_thread")]
async fn russet_probe_builds_in_release() {
    let Some(h) = host().await else { return };
    let (_dir, root) = support::tmp();
    let project = support::copy_probe(&root);
    let b = builder(&h, &root);
    let sink = RecordingSink::default();
    let mut r = BuildRequest::new(&project);
    r.configuration = Some("release".into());
    let out = b.build(r, &sink, CancellationToken::new()).await.unwrap_or_else(|e| {
        let lines = sink.lines.lock().unwrap();
        panic!("release build failed: {e:?}\n{}", lines.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n"))
    });
    assert_eq!(out.app.path, project.join(".russet/build/release-iphoneos/RussetProbe.app"));
    let text = otool(&h, &out.app.executable);
    assert!(text.contains("sdk 27.0") && text.contains("minos 17.0"), "{text}");
    assert!(out.signing_plan.items.iter().all(|i| !i.get_task_allow), "release builds are not debuggable");
    assert_eq!(out.dsyms.len(), 1, "release builds have a dSYM");
    let dwarf = out.dsyms[0].join("Contents/Resources/DWARF/RussetProbe");
    assert!(dwarf.is_file(), "the dSYM's DWARF file is named after the executable: {dwarf}");
    for l in &out.lints {
        eprintln!("lint {} {:?} {} (fixed: {})", l.code, l.severity, l.message, l.fixed);
    }
}

/// A package dylib (a local path dependency with a `.dynamic` product) links with the SDK's version too: the
/// toolset carries the link-time SDK fix for every link, so `Frameworks/libKit.dylib` says `sdk 27.0`. Such a graph
/// builds with the `native` backend (Swift Build cannot link `.dynamic` products on Linux, build §4.1), and the
/// dependency is described in its own scratch under `.russet/work`, never writing `.build/` into its directory.
#[tokio::test(flavor = "multi_thread")]
async fn a_package_dylib_links_with_the_sdk_version() {
    let Some(h) = host().await else { return };
    let (_dir, root) = support::tmp();
    let project = support::copy_probe(&root);
    let kit = root.join("Kit");
    std::fs::create_dir_all(kit.join("Sources/Kit")).unwrap();
    std::fs::write(
        kit.join("Package.swift"),
        "// swift-tools-version: 6.0\nimport PackageDescription\nlet package = Package(name: \"Kit\", platforms: \
         [.iOS(.v17)], products: [.library(name: \"Kit\", type: .dynamic, targets: [\"Kit\"])], targets: \
         [.target(name: \"Kit\")])\n",
    )
    .unwrap();
    std::fs::write(kit.join("Sources/Kit/Kit.swift"), "public func kitGreeting() -> String { \"kit\" }\n").unwrap();
    let manifest = std::fs::read_to_string(project.join("Package.swift")).unwrap();
    let manifest = manifest
        .replace("    targets: [\n        .target(name: \"RussetProbe\")", "    dependencies: [.package(path: \"../Kit\")],\n    targets: [\n        .target(name: \"RussetProbe\", dependencies: [\"Kit\"])");
    assert!(manifest.contains("../Kit"), "{manifest}");
    std::fs::write(project.join("Package.swift"), manifest).unwrap();
    let app = project.join("Sources/RussetProbe/KitUse.swift");
    std::fs::write(&app, "import Kit\n\npublic let kitText = kitGreeting()\n").unwrap();

    let b = builder(&h, &root);
    let sink = RecordingSink::default();
    let t0 = Instant::now();
    let out = b.build(BuildRequest::new(&project), &sink, CancellationToken::new()).await.unwrap_or_else(|e| {
        let lines = sink.lines.lock().unwrap();
        panic!("build failed: {e:?}\n{}", lines.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n"))
    });
    eprintln!("native-backend build with a package dylib: {:.1} s", t0.elapsed().as_secs_f64());
    let lines = sink.lines.lock().unwrap().clone();
    assert!(lines.iter().any(|(_, l)| l.contains("build.warn.backend")), "the native fallback is announced");

    let dylib = out.app.path.join("Frameworks/libKit.dylib");
    assert!(dylib.is_file(), "the package dylib is embedded");
    let text = otool(&h, &dylib);
    let build_version: Vec<&str> =
        text.lines().skip_while(|l| !l.contains("LC_BUILD_VERSION")).take(6).map(str::trim).collect();
    eprintln!("libKit.dylib: {}", build_version.join(" | "));
    assert!(text.contains("platform 2"), "{text}");
    assert!(text.contains("sdk 27.0"), "LC_BUILD_VERSION sdk of the package dylib: {text}");
    assert!(text.contains("minos 17.0"), "LC_BUILD_VERSION minos of the package dylib: {text}");
    let exe = otool(&h, &out.app.executable);
    assert!(exe.contains("sdk 27.0") && exe.contains("minos 17.0"), "{exe}");
    assert!(exe.contains("@rpath/libKit.dylib"), "the app loads the dylib through @rpath");
    for file in [&dylib, &out.app.executable] {
        for r in rpaths(&otool(&h, file)) {
            assert!(r.starts_with('@') || r == "/usr/lib/swift", "host rpath left in {file}: {r}");
        }
    }
    // The dylib is signed before the app, which comes last.
    let items: Vec<(&Utf8Path, SignKind)> = out.signing_plan.items.iter().map(|i| (i.path.as_path(), i.kind)).collect();
    assert!(items.contains(&(dylib.as_path(), SignKind::MachO)), "{items:?}");
    assert_eq!(items.last(), Some(&(out.app.path.as_path(), SignKind::Bundle)));
    // The report lists it under the app bundle, relative to the bundle.
    let report: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out.report_path).unwrap()).unwrap();
    assert_eq!(report["project"]["backend"], "native");
    let machos = report["bundles"][0]["machO"].as_array().unwrap();
    let entry = machos.iter().find(|m| m["path"] == "Frameworks/libKit.dylib").unwrap();
    assert_eq!(entry["sdk"], "27.0");
    assert_eq!(entry["builtByRusset"], true);
    // SwiftPM wrote nothing into the dependency's own directory.
    assert!(!kit.join(".build").exists(), "describe/build wrote .build into the local dependency");
    assert!(project.join(".russet/work/describe/kit").is_dir(), "the dependency's describe scratch");
}
