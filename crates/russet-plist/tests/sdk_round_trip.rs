// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! B01 acceptance on real Apple files: the SDK's own `SDKSettings.plist` (binary), the platform `Info.plist`
//! (XML in xtool's bundle, binary in the Xcode copy) and the version plists re-serialize byte for byte.
//!
//! Local and env-gated (PLAN §0 rule 9): set `RUSSET_TEST_SDK_BUNDLE` to a Darwin SDK bundle such as
//! `~/.swiftpm/swift-sdks/darwin.artifactbundle`. The files are read in place and never copied or committed.

use std::path::{Path, PathBuf};

fn bundle() -> Option<PathBuf> {
    std::env::var_os("RUSSET_TEST_SDK_BUNDLE").map(PathBuf::from)
}

fn round_trips(path: &Path) {
    let original = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (value, format) = russet_plist::from_bytes(&original).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let written = russet_plist::to_bytes(&value, format).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(written == original, "{} ({format:?}) does not re-serialize byte for byte", path.display());
}

#[test]
fn sdk_plists_round_trip_byte_for_byte() {
    let Some(bundle) = bundle() else {
        eprintln!("skipped: set RUSSET_TEST_SDK_BUNDLE to a Darwin SDK bundle");
        return;
    };
    let platform = bundle.join("Developer/Platforms/iPhoneOS.platform");
    let sdks = platform.join("Developer/SDKs");
    let sdk = std::fs::read_dir(&sdks)
        .expect("SDKs directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x == "sdk"))
        .expect("an iPhoneOS SDK");
    let mut files = vec![
        sdk.join("SDKSettings.plist"),
        sdk.join("System/Library/CoreServices/SystemVersion.plist"),
        platform.join("Info.plist"),
    ];
    let xcode = bundle.join("Xcode.app/Contents");
    for extra in [
        "version.plist",
        "Developer/Platforms/iPhoneOS.platform/version.plist",
        "Developer/Platforms/iPhoneOS.platform/Info.plist",
    ] {
        let p = xcode.join(extra);
        if p.exists() {
            files.push(p);
        }
    }
    for f in &files {
        round_trips(f);
    }
}
