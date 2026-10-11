// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! PLAN §4.5 tests on this host: RussetProbe builds with Russet's SwiftPM backend (`xtool` hidden from `PATH`),
//! `llvm-otool -l` shows `sdk 27.0` and `minos 17.0`, no host `LC_RPATH` is left, the scene manifest is present,
//! two clean builds are byte-identical, the IPA is deterministic with modes 755/644, and a warm build is fast.
//!
//! Local and env-gated: set `RUSSET_TEST_SDK_BUNDLE` to a Darwin SDK bundle (for example
//! `~/.swiftpm/swift-sdks/darwin.artifactbundle`, read in place). Needs Swift 6.4 on `PATH` and LLVM ≥ 23's
//! `llvm-otool` and `llvm-install-name-tool` (they stand in for W6's russet-macho bodies until those land).

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use camino::{Utf8Path, Utf8PathBuf};
use plist::Value;
use russet_build::{BuildEnv, BuildRequest, Builder, PackageKind};
use russet_sdk::XtoolBundleProvider;
use russet_sdk::host::HostToolchain;
use support::{LlvmMachO, LocalRunner, RecordingSink};
use tokio_util::sync::CancellationToken;

struct Host {
    bundle: Utf8PathBuf,
    macho: LlvmMachO,
    host: HostToolchain,
}

fn host() -> Option<Host> {
    let Some(bundle) = std::env::var_os("RUSSET_TEST_SDK_BUNDLE") else {
        eprintln!("skipped: set RUSSET_TEST_SDK_BUNDLE to a Darwin SDK bundle");
        return None;
    };
    let bundle = Utf8PathBuf::from_path_buf(bundle.into()).unwrap();
    let Some(macho) = LlvmMachO::find() else {
        eprintln!("skipped: needs llvm-otool and llvm-install-name-tool from LLVM 23 or newer");
        return None;
    };
    Some(Host { bundle, macho, host: HostToolchain::probe().unwrap() })
}

fn builder(h: Host, root: &Utf8Path) -> Builder {
    let data = root.join("data");
    let provider =
        XtoolBundleProvider::new(&data, h.host).with_search_dirs(vec![h.bundle.parent().unwrap().to_owned()]);
    let env = BuildEnv::new(Arc::new(provider), &data, root.join("cache"), Arc::new(LocalRunner))
        .with_macho(Arc::new(h.macho));
    Builder::new(env).unwrap()
}

fn otool(path: &Utf8Path) -> String {
    let otool = HostToolchain::probe().unwrap().llvm_otool.unwrap();
    let out = std::process::Command::new(otool.as_std_path()).arg("-l").arg(path.as_str()).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
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
    let Some(h) = host() else { return };
    let (_dir, root) = support::tmp();
    let project = support::copy_probe(&root);
    let b = builder(h, &root);
    let sink = RecordingSink::default();

    let t0 = Instant::now();
    let out = b.build(BuildRequest::new(&project), &sink, CancellationToken::new()).await.unwrap_or_else(|e| {
        let lines = sink.lines.lock().unwrap();
        panic!("build failed: {e:?}\n{}", lines.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n"))
    });
    eprintln!("cold build: {:.1} s", t0.elapsed().as_secs_f64());

    // sdk 27.0 / minos 17.0 on the executable (and on every Mach-O Russet built).
    let text = otool(&out.app.executable);
    assert!(text.contains("platform 2"), "{text}");
    assert!(text.contains("sdk 27.0"), "LC_BUILD_VERSION sdk: {text}");
    assert!(text.contains("minos 17.0"), "LC_BUILD_VERSION minos: {text}");
    // No absolute host rpath is left.
    let rpaths: Vec<&str> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("path "))
        .map(|p| p.split(" (offset").next().unwrap_or(p))
        .collect();
    assert!(!rpaths.is_empty());
    for r in &rpaths {
        assert!(r.starts_with('@') || *r == "/usr/lib/swift", "host rpath left: {r} in {rpaths:?}");
    }
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

fn sha(path: &Utf8Path) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(std::fs::read(path).unwrap()).iter().map(|b| format!("{b:02x}")).collect()
}
