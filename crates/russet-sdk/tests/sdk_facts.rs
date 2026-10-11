// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! SDK facts, selection, the platform view and the toolsets on synthetic bundles written at test time (no Apple
//! files), plus an env-gated read of a real bundle (`RUSSET_TEST_SDK_BUNDLE`, never committed).

#![allow(clippy::unwrap_used)]

use camino::{Utf8Path, Utf8PathBuf};
use plist::{Dictionary, Value};
use russet_core::error::codes;
use russet_sdk::facts::BundleFacts;
use russet_sdk::host::HostToolchain;
use russet_sdk::{SdkProvider, SdkSelector, Version, XtoolBundleProvider};

fn dict(pairs: &[(&str, Value)]) -> Dictionary {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
}

fn s(v: &str) -> Value {
    Value::String(v.to_owned())
}

fn write_plist(path: &Utf8Path, d: Dictionary, binary: bool) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let v = Value::Dictionary(d);
    let bytes = if binary { russet_plist::to_binary(&v) } else { russet_plist::to_xml(&v) }.unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A Darwin SDK bundle in xtool's normal layout, with made-up content.
fn make_bundle(root: &Utf8Path, name: &str, xcode: &str, xcode_build: &str, sdk: &str) -> Utf8PathBuf {
    let b = root.join(name);
    let platform = b.join("Developer/Platforms/iPhoneOS.platform");
    let sdk_dir = platform.join(format!("Developer/SDKs/iPhoneOS{sdk}.sdk"));
    let targets = Value::Array(["15.0", "17.0", sdk].iter().map(|v| s(v)).collect());
    let iphoneos = dict(&[
        ("ValidDeploymentTargets", targets),
        (
            "SwiftCompatibilityLibraryNotNeededStartingIn",
            Value::Dictionary(dict(&[("libswiftCompatibilitySpan.dylib", s("26.0"))])),
        ),
    ]);
    write_plist(
        &sdk_dir.join("SDKSettings.plist"),
        dict(&[
            ("Version", s(sdk)),
            ("CanonicalName", s(&format!("iphoneos{sdk}"))),
            ("DefaultProperties", Value::Dictionary(dict(&[("DEFAULT_COMPILER", s("com.example.compiler"))]))),
            ("SupportedTargets", Value::Dictionary(dict(&[("iphoneos", Value::Dictionary(iphoneos))]))),
        ]),
        true,
    );
    write_plist(
        &sdk_dir.join("System/Library/CoreServices/SystemVersion.plist"),
        dict(&[("ProductBuildVersion", s("24A430"))]),
        false,
    );
    let iface = sdk_dir.join("usr/lib/swift/Swift.swiftmodule/arm64e-apple-ios.swiftinterface");
    std::fs::create_dir_all(iface.parent().unwrap()).unwrap();
    std::fs::write(&iface, "// swift-interface-format-version: 1.0\n// swift-compiler-version: Apple Swift version 6.4 effective-5.10 (x)\n").unwrap();
    let additional = dict(&[
        ("DTPlatformName", s("iphoneos")),
        ("DTPlatformVersion", s(sdk)),
        ("DTSDKName", s("$(SDK_NAME)")),
        ("DTSDKBuild", s("$(SDK_PRODUCT_BUILD_VERSION)")),
        ("DTPlatformBuild", s("$(PLATFORM_PRODUCT_BUILD_VERSION)")),
    ]);
    write_plist(
        &platform.join("Info.plist"),
        dict(&[("AdditionalInfo", Value::Dictionary(additional)), ("Version", s(sdk))]),
        false,
    );
    let contents = b.join("Xcode.app/Contents");
    write_plist(
        &contents.join("version.plist"),
        dict(&[("CFBundleShortVersionString", s(xcode)), ("ProductBuildVersion", s(xcode_build))]),
        false,
    );
    write_plist(
        &contents.join("Developer/Platforms/iPhoneOS.platform/version.plist"),
        dict(&[("ProductBuildVersion", s("24A430"))]),
        false,
    );
    std::fs::create_dir_all(contents.join("Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/iphoneos"))
        .unwrap();
    std::fs::create_dir_all(contents.join("Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-6.2/iphoneos"))
        .unwrap();
    std::fs::create_dir_all(b.join("Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift")).unwrap();
    std::fs::create_dir_all(b.join("toolset/bin")).unwrap();
    for tool in ["ld64.lld", "llvm-lib", "dsymutil"] {
        std::fs::write(b.join("toolset/bin").join(tool), "#!/bin/sh\n").unwrap();
    }
    std::fs::write(b.join("darwin-sdk-version.txt"), "epoch=3,darwinTools=1.1.0,oam=1.3.0\n").unwrap();
    b
}

fn host() -> HostToolchain {
    HostToolchain {
        swift: "/opt/swift/bin/swift".into(),
        bin_dir: "/opt/swift/bin".into(),
        version: Some(Version::new(6, 4, 0)),
        version_text: "Swift version 6.4 (swift-6.4-RELEASE)".into(),
        llvm_strip: None,
        llvm_lipo: None,
        llvm_otool: None,
        llvm_install_name_tool: None,
    }
}

fn tmp() -> (tempfile::TempDir, Utf8PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    (dir, path)
}

#[test]
fn bundle_facts_come_from_the_sdk_files() {
    let (_g, root) = tmp();
    let b = make_bundle(&root, "darwin.artifactbundle", "27.0", "27A266a", "27.0");
    let f = BundleFacts::read(&b).unwrap();
    assert_eq!(f.id(), "xcode-27.0-27A266a");
    assert_eq!(f.sdk_version, Version::new(27, 0, 0));
    assert_eq!(f.sdk_canonical_name, "iphoneos27.0");
    assert_eq!(f.sdk_build.as_deref(), Some("24A430"));
    assert_eq!(f.platform_build.as_deref(), Some("24A430"));
    assert_eq!(f.xcode_version, Version::new(27, 0, 0));
    assert_eq!(f.swift_required, Version::new(6, 4, 0));
    assert_eq!(
        f.valid_deployment_targets,
        vec![Version::new(15, 0, 0), Version::new(17, 0, 0), Version::new(27, 0, 0)]
    );
    assert_eq!(f.default_compiler.as_deref(), Some("com.example.compiler"));
    assert_eq!(f.back_deploy_not_needed_from.get("libswiftCompatibilitySpan.dylib"), Some(&Version::new(26, 0, 0)));
    assert_eq!(f.back_deploy_dirs.len(), 2);
    assert!(f.back_deploy_dirs[0].ends_with("swift-5.5/iphoneos"));
    assert_eq!(f.darwin_tools.as_deref(), Some("1.1.0"));
    assert_eq!(f.linker_dir, Some(b.join("toolset/bin")));
    assert!(f.sdk_path.ends_with("iPhoneOS27.0.sdk"));
    assert_eq!(f.additional_info.get("DTSDKName").and_then(Value::as_string), Some("$(SDK_NAME)"));
}

#[test]
fn a_missing_xcode_version_plist_is_named() {
    let (_g, root) = tmp();
    let b = make_bundle(&root, "slim.artifactbundle", "27.0", "27A266a", "27.0");
    std::fs::remove_file(b.join("Xcode.app/Contents/version.plist")).unwrap();
    let err = BundleFacts::read(&b).unwrap_err();
    assert_eq!(err.code(), codes::sdk::INCOMPLETE);
    assert!(err.message().contains("Xcode.app/Contents/version.plist"), "{}", err.message());
    assert!(err.hint().is_some());
}

#[tokio::test]
async fn resolve_writes_the_view_and_toolsets() {
    let (_g, root) = tmp();
    let sdks = root.join("swift-sdks");
    let b = make_bundle(&sdks, "darwin.artifactbundle", "27.0", "27A266a", "27.0");
    let data = root.join("data");
    let tool = root.join("russet-tool");
    std::fs::write(&tool, "").unwrap();
    let p = XtoolBundleProvider::new(&data, host()).with_search_dirs(vec![sdks.clone()]).with_tool(Some(tool.clone()));
    let facts = p.resolve(&SdkSelector::Auto).await.unwrap();
    assert_eq!(facts.id, "xcode-27.0-27A266a");
    assert_eq!(facts.platforms_view, data.join("sdk/xcode-27.0-27A266a/view/Developer/Platforms"));
    assert_eq!(facts.tools.ld64_lld, b.join("toolset/bin/ld64.lld"));
    assert_eq!(facts.tools.librarian, b.join("toolset/bin/llvm-lib"));
    assert_eq!(facts.tools.swift_version, Some(Version::new(6, 4, 0)));
    let toolset: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&facts.toolset_swb).unwrap()).unwrap();
    assert_eq!(toolset["swiftCompiler"]["extraCLIOptions"][3], facts.sdk_path.as_str());
    assert_eq!(toolset["rootPath"], b.join("toolset/bin").as_str());
    let native = std::fs::read_to_string(&facts.toolset_native).unwrap();
    assert!(native.contains("-enable-cross-import-overlays"));
    #[cfg(unix)]
    {
        let platform = facts.platforms_view.join("iPhoneOS.platform");
        assert_eq!(
            std::fs::read_link(platform.join("Info.plist")).unwrap(),
            b.join("Developer/Platforms/iPhoneOS.platform/Info.plist")
        );
        assert_eq!(
            std::fs::read_link(platform.join("Developer")).unwrap(),
            b.join("Developer/Platforms/iPhoneOS.platform/Developer")
        );
        assert_eq!(
            std::fs::read_link(platform.join("version.plist")).unwrap(),
            b.join("Xcode.app/Contents/Developer/Platforms/iPhoneOS.platform/version.plist")
        );
        assert_eq!(std::fs::read_link(platform.join("usr/local/bin/actool")).unwrap(), tool);
        assert_eq!(
            std::fs::read_link(facts.platforms_view.join("../Toolchains")).unwrap(),
            b.join("Developer/Toolchains")
        );
        // Swift Build derives SWIFT_RESOURCE_DIR as $(PLATFORM_DIR)/../../Toolchains/…: it must resolve.
        assert!(platform.join("../../Toolchains/XcodeDefault.xctoolchain/usr/lib/swift").is_dir());
        // A second resolve keeps the view; a different tool rebuilds it.
        let marker = std::fs::read_to_string(data.join("sdk/xcode-27.0-27A266a/view.json")).unwrap();
        p.resolve(&SdkSelector::Auto).await.unwrap();
        assert_eq!(std::fs::read_to_string(data.join("sdk/xcode-27.0-27A266a/view.json")).unwrap(), marker);
        let p2 = p.clone().with_tool(None);
        let again = p2.resolve(&SdkSelector::Auto).await.unwrap();
        assert!(!again.platforms_view.join("iPhoneOS.platform/usr/local/bin/actool").exists());
        assert!(again.platforms_view.join("iPhoneOS.platform/usr/local/bin").is_dir());
        let leftovers: Vec<String> = std::fs::read_dir(data.join("sdk/xcode-27.0-27A266a"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-") || n.contains(".old-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}

#[tokio::test]
async fn selection_prefers_the_newest_and_honours_selectors() {
    let (_g, root) = tmp();
    let sdks = root.join("swift-sdks");
    make_bundle(&sdks, "a.artifactbundle", "26.4", "26E100", "26.4");
    make_bundle(&sdks, "b.artifactbundle", "27.0", "27A266a", "27.0");
    std::fs::create_dir_all(sdks.join("linux.artifactbundle")).unwrap(); // not a Darwin bundle: ignored
    let p = XtoolBundleProvider::new(root.join("data"), host()).with_search_dirs(vec![sdks.clone()]);
    assert_eq!(p.inspect(&SdkSelector::Auto).unwrap().xcode_build, "27A266a");
    assert_eq!(p.inspect(&SdkSelector::XcodeBuild("26e100".into())).unwrap().sdk_canonical_name, "iphoneos26.4");
    assert_eq!(p.inspect(&SdkSelector::CanonicalName("iphoneos27.0".into())).unwrap().xcode_build, "27A266a");
    assert_eq!(p.inspect(&SdkSelector::CanonicalName("iphoneos99.0".into())).unwrap_err().code(), codes::sdk::MISSING);
    let empty = XtoolBundleProvider::new(root.join("data"), host()).with_search_dirs(vec![root.join("nowhere")]);
    let err = empty.resolve(&SdkSelector::Auto).await.unwrap_err();
    assert_eq!(err.code(), codes::sdk::MISSING);
    assert!(err.hint().is_some_and(|h| h.contains("sdk import")));
}

#[tokio::test]
async fn a_bundle_without_darwin_tools_cannot_link() {
    let (_g, root) = tmp();
    let sdks = root.join("swift-sdks");
    let b = make_bundle(&sdks, "darwin.artifactbundle", "27.0", "27A266a", "27.0");
    std::fs::remove_dir_all(b.join("toolset")).unwrap();
    let p = XtoolBundleProvider::new(root.join("data"), host()).with_search_dirs(vec![sdks]);
    assert_eq!(p.resolve(&SdkSelector::Auto).await.unwrap_err().code(), codes::toolchain::LINKER_UNSUPPORTED);
}

/// `sdk.buildFacts` passes on a complete bundle and fails when a default build cannot work: no platform
/// `version.plist` (DTPlatformBuild under `metadata = "sdk"`) or no darwin-tools `ld64.lld`.
#[tokio::test]
async fn the_build_facts_check_fails_when_a_build_cannot_work() {
    use russet_doctor::{Check, CheckCx, Status};
    let cx = CheckCx { project: None, udid: None, online: false };
    let (_g, root) = tmp();
    let sdks = root.join("swift-sdks");
    let b = make_bundle(&sdks, "darwin.artifactbundle", "27.0", "27A266a", "27.0");
    let check = russet_sdk::BuildFactsCheck::with_search_dirs(vec![sdks.clone()]);
    assert_eq!(check.id(), "sdk.buildFacts");
    let ok = check.run(&cx).await;
    assert_eq!(ok.status, Status::Pass, "{ok:?}");
    assert!(ok.summary.contains("xcode-27.0-27A266a"));

    std::fs::remove_file(b.join("Xcode.app/Contents/Developer/Platforms/iPhoneOS.platform/version.plist")).unwrap();
    let no_version = check.run(&cx).await;
    assert_eq!(no_version.status, Status::Fail, "{no_version:?}");
    assert!(no_version.summary.contains("version.plist"));
    assert!(no_version.hint.is_some());

    let b2 = make_bundle(&root.join("other"), "darwin.artifactbundle", "27.0", "27A266a", "27.0");
    std::fs::remove_file(b2.join("toolset/bin/ld64.lld")).unwrap();
    let no_linker = russet_sdk::BuildFactsCheck::with_search_dirs(vec![root.join("other")]).run(&cx).await;
    assert_eq!(no_linker.status, Status::Fail, "{no_linker:?}");
    assert!(no_linker.summary.contains("ld64.lld"));

    let none = russet_sdk::BuildFactsCheck::with_search_dirs(vec![root.join("nowhere")]).run(&cx).await;
    assert_eq!(none.status, Status::Fail);
    assert!(none.hint.is_some());
}

/// Local only: a real Darwin SDK bundle (`RUSSET_TEST_SDK_BUNDLE`), read in place.
#[test]
fn a_real_bundle_reads_completely() {
    let Some(bundle) = std::env::var_os("RUSSET_TEST_SDK_BUNDLE") else {
        eprintln!("skipped: set RUSSET_TEST_SDK_BUNDLE to a Darwin SDK bundle");
        return;
    };
    let bundle = Utf8PathBuf::from_path_buf(bundle.into()).unwrap();
    let f = BundleFacts::read(&bundle).unwrap();
    assert!(f.sdk_canonical_name.starts_with("iphoneos"));
    assert!(f.sdk_build.is_some() && f.platform_build.is_some());
    assert!(!f.back_deploy_dirs.is_empty());
    assert!(f.linker_dir.is_some());
    assert!(f.valid_deployment_targets.contains(&Version::new(17, 0, 0)));
    eprintln!(
        "{} {} SDK {} ({:?}), Swift {}",
        f.id(),
        f.sdk_canonical_name,
        f.sdk_version,
        f.sdk_build,
        f.swift_required
    );
}
