// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Test fixtures generated at test time: fake tools (shell scripts), a fake xtool-style SDK bundle, fake Pythons
//! and a signed pin manifest. Nothing here is an Apple file; every plist and JSON is written by the test.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use russet_toolchain::host::HostEnv;
use russet_toolchain::pins::PinManifest;

/// Writes an executable shell script.
pub fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// A fake Swift toolchain at `root` whose `swift --version` prints `version`; returns `<root>/usr/bin`.
pub fn swift_toolchain(root: &Path, version: &str) -> PathBuf {
    let bin = root.join("usr/bin");
    script(&bin.join("swift"), &format!("echo 'Swift version {version} (swift-{version}-RELEASE)'"));
    bin
}

/// A fake xtool-style SDK bundle for Xcode `xcode` (build `build`) carrying darwin-tools `darwin_tools` and a
/// linker that reports `lld`.
pub fn sdk_bundle(dir: &Path, xcode: &str, build: &str, darwin_tools: &str, lld: &str) -> PathBuf {
    let b = dir.join("darwin.artifactbundle");
    std::fs::create_dir_all(&b).expect("bundle");
    std::fs::write(
        b.join("info.json"),
        r#"{"schemaVersion":"1.0","artifacts":{"darwin":{"type":"swiftSDK","version":"0.0.1","variants":[{"path":".","supportedTriples":["x86_64-unknown-linux-gnu"]}]}}}"#,
    )
    .expect("info.json");
    let sdk_root = format!("Developer/Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS{xcode}.sdk");
    std::fs::write(
        b.join("swift-sdk.json"),
        format!(r#"{{"schemaVersion":"4.0","targetTriples":{{"arm64-apple-ios":{{"sdkRootPath":"{sdk_root}"}}}}}}"#),
    )
    .expect("swift-sdk.json");
    std::fs::create_dir_all(b.join(&sdk_root)).expect("sdk root");
    std::fs::write(
        b.join(&sdk_root).join("SDKSettings.json"),
        format!(r#"{{"CanonicalName":"iphoneos{xcode}","Version":"{xcode}"}}"#),
    )
    .expect("SDKSettings.json");
    std::fs::create_dir_all(b.join("Xcode.app/Contents")).expect("Xcode.app");
    std::fs::write(
        b.join("Xcode.app/Contents/version.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>CFBundleShortVersionString</key><string>{xcode}</string>\
             <key>ProductBuildVersion</key><string>{build}</string></dict></plist>\n"
        ),
    )
    .expect("version.plist");
    std::fs::write(b.join("darwin-sdk-version.txt"), format!("epoch=3,darwinTools={darwin_tools},oam=1.3.0\n"))
        .expect("darwin-sdk-version.txt");
    std::fs::write(
        b.join("toolset.json"),
        r#"{"schemaVersion":"1.0","rootPath":"toolset/bin","linker":{"path":"ld64.lld"}}"#,
    )
    .expect("toolset.json");
    script(&b.join("toolset/bin/ld64.lld"), &format!("echo 'LLD {lld}'"));
    b
}

/// A host that sees only `path` and `sdk_dirs` (no real uv, swiftly or ldconfig).
pub fn host(path: Vec<PathBuf>, sdk_dirs: Vec<PathBuf>, home: &Path) -> HostEnv {
    HostEnv {
        path,
        home: Some(home.to_path_buf()),
        swift_sdk_dirs: sdk_dirs,
        uv_python_dirs: Vec::new(),
        swiftly_dirs: Vec::new(),
        ldconfig: None,
    }
}

/// A test-time minisign key pair: (public key file text, secret key).
pub fn keypair() -> (String, minisign::SecretKey) {
    let kp = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
    let pk = kp.pk.to_box().expect("pk box").to_string();
    (pk, kp.sk)
}

/// Signs `text` with `sk` (prehashed, as minisign does by default).
pub fn sign(sk: &minisign::SecretKey, text: &str) -> String {
    let sig =
        minisign::sign(None, sk, std::io::Cursor::new(text.as_bytes()), Some("russet test pins"), None).expect("sign");
    sig.to_string()
}

/// A manifest with the Xcode 27 row and the given components (TOML snippets), signed by a fresh key.
pub fn signed_manifest(components: &str) -> (PinManifest, String, String, String) {
    let text = format!(
        "schema = 1\nid = \"test\"\nmin-russet = \"0.1.0\"\ngenerated-at = \"2026-10-11T00:00:00Z\"\n\n\
         [[xcode]]\nversion = \"27.0\"\nbuild = \"27A266a\"\nios-sdk = \"27.0\"\nswift = \"6.4\"\nmin-lld = \"23.1.3\"\n\
         min-darwin-tools = \"1.1.0\"\n\n{components}"
    );
    let (pk, sk) = keypair();
    let sig = sign(&sk, &text);
    let m = PinManifest::load_verified(&text, &sig, &[&pk]).expect("the test manifest verifies");
    (m, text, sig, pk)
}
