// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! B07: IPA listing, modes, determinism, compression modes and `SwiftSupport/` on a synthetic bundle built at
//! test time (no Apple files).

#![allow(clippy::unwrap_used)]

use std::time::{Duration, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use russet_ipa::{Compression, IpaMode, IpaOptions};

/// A minimal thin arm64 Mach-O header (magic, CPU type arm64, subtype, MH_EXECUTE) followed by padding.
fn fake_macho() -> Vec<u8> {
    let mut b = vec![0xCF, 0xFA, 0xED, 0xFE, 0x0C, 0x00, 0x00, 0x01, 0, 0, 0, 0, 2, 0, 0, 0];
    b.resize(256, 0);
    b
}

fn make_app(root: &Utf8Path) -> Utf8PathBuf {
    let app = root.join("Probe.app");
    std::fs::create_dir_all(app.join("Frameworks/Lib.framework")).unwrap();
    std::fs::create_dir_all(app.join("Probe_Probe.bundle")).unwrap();
    std::fs::write(app.join("Probe"), fake_macho()).unwrap();
    std::fs::write(app.join("Info.plist"), b"bplist00 not really").unwrap();
    std::fs::write(app.join("PkgInfo"), b"APPL????").unwrap();
    std::fs::write(app.join("Frameworks/Lib.framework/Lib"), fake_macho()).unwrap();
    std::fs::write(app.join("Frameworks/Lib.framework/Info.plist"), b"<plist/>").unwrap();
    std::fs::write(app.join("Probe_Probe.bundle/data.txt"), "x".repeat(10_000)).unwrap();
    std::fs::write(app.join("empty"), b"").unwrap();
    app
}

fn opts(mode: IpaMode) -> IpaOptions {
    IpaOptions { mode, swift_support: None, epoch: UNIX_EPOCH + Duration::from_secs(978_307_200) }
}

fn tmp() -> (tempfile::TempDir, Utf8PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    (dir, path)
}

#[test]
fn listing_is_sorted_with_explicit_directories_and_modes_from_magic() {
    let (_guard, root) = tmp();
    let app = make_app(&root);
    let out = root.join("out/Probe.ipa");
    let report = russet_ipa::write(&app, &out, opts(IpaMode::Dev)).unwrap();
    assert_eq!(report.path, out);
    let listing = russet_ipa::read_listing(&out).unwrap();
    assert_eq!(report.entries, listing.len());
    let names: Vec<&str> = listing.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Payload/",
            "Payload/Probe.app/",
            "Payload/Probe.app/Frameworks/",
            "Payload/Probe.app/Frameworks/Lib.framework/",
            "Payload/Probe.app/Frameworks/Lib.framework/Info.plist",
            "Payload/Probe.app/Frameworks/Lib.framework/Lib",
            "Payload/Probe.app/Info.plist",
            "Payload/Probe.app/PkgInfo",
            "Payload/Probe.app/Probe",
            "Payload/Probe.app/Probe_Probe.bundle/",
            "Payload/Probe.app/Probe_Probe.bundle/data.txt",
            "Payload/Probe.app/empty",
        ]
    );
    let mode = |n: &str| listing.iter().find(|e| e.name == n).unwrap().unix_mode;
    assert_eq!(mode("Payload/"), 0o040755);
    assert_eq!(mode("Payload/Probe.app/Probe"), 0o100755);
    assert_eq!(mode("Payload/Probe.app/Frameworks/Lib.framework/Lib"), 0o100755);
    assert_eq!(mode("Payload/Probe.app/Info.plist"), 0o100644);
    assert!(listing.iter().all(|e| e.compression == Compression::Stored));
    assert_eq!(russet_ipa::app_dir(&listing).as_deref(), Some("Payload/Probe.app/"));
    assert_eq!(russet_ipa::read_entry(&out, "Payload/Probe.app/PkgInfo").unwrap(), b"APPL????");
    assert!(russet_ipa::read_entry(&out, "Payload/missing").is_err());
}

#[test]
fn the_exec_bit_comes_from_the_magic_not_the_filesystem() {
    let (_guard, root) = tmp();
    let app = make_app(&root);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = app.join("script.sh");
        std::fs::write(&script, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(app.join("Probe"), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let out = root.join("Probe.ipa");
    russet_ipa::write(&app, &out, opts(IpaMode::Dev)).unwrap();
    let listing = russet_ipa::read_listing(&out).unwrap();
    let mode = |n: &str| listing.iter().find(|e| e.name == n).map(|e| e.unix_mode);
    assert_eq!(mode("Payload/Probe.app/Probe"), Some(0o100755));
    #[cfg(unix)]
    assert_eq!(mode("Payload/Probe.app/script.sh"), Some(0o100644));
}

#[test]
fn two_runs_give_the_same_bytes() {
    let (_guard, root) = tmp();
    let app = make_app(&root);
    for mode in [IpaMode::Dev, IpaMode::Store] {
        let a = russet_ipa::write(&app, &root.join("a.ipa"), opts(mode)).unwrap();
        // Touch the sources: file times must not leak into the archive.
        std::fs::write(app.join("PkgInfo"), b"APPL????").unwrap();
        let b = russet_ipa::write(&app, &root.join("b.ipa"), opts(mode)).unwrap();
        assert_eq!(a.sha256, b.sha256, "{mode:?}");
        assert_eq!(std::fs::read(&a.path).unwrap(), std::fs::read(&b.path).unwrap());
    }
}

#[test]
fn store_mode_deflates_and_dev_mode_stores() {
    let (_guard, root) = tmp();
    let app = make_app(&root);
    let dev = russet_ipa::write(&app, &root.join("dev.ipa"), opts(IpaMode::Dev)).unwrap();
    let store = russet_ipa::write(&app, &root.join("store.ipa"), opts(IpaMode::Store)).unwrap();
    assert!(store.bytes < dev.bytes, "deflate shrinks the 10 kB text file");
    let listing = russet_ipa::read_listing(&store.path).unwrap();
    let entry = |n: &str| listing.iter().find(|e| e.name == n).unwrap();
    assert_eq!(entry("Payload/Probe.app/Probe_Probe.bundle/data.txt").compression, Compression::Deflated);
    assert_eq!(entry("Payload/Probe.app/Probe_Probe.bundle/data.txt").size, 10_000);
    assert_eq!(entry("Payload/Probe.app/empty").compression, Compression::Stored);
    assert_eq!(entry("Payload/").compression, Compression::Stored);
}

#[test]
fn swift_support_is_added_unmodified() {
    let (_guard, root) = tmp();
    let app = make_app(&root);
    let lib = root.join("libswiftCompatibilitySpan.dylib");
    std::fs::write(&lib, fake_macho()).unwrap();
    let mut o = opts(IpaMode::Store);
    o.swift_support = Some(vec![lib.clone()]);
    let out = russet_ipa::write(&app, &root.join("store.ipa"), o).unwrap();
    let listing = russet_ipa::read_listing(&out.path).unwrap();
    let names: Vec<&str> = listing.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"SwiftSupport/iphoneos/libswiftCompatibilitySpan.dylib"));
    assert!(names.contains(&"SwiftSupport/"));
    let bytes = russet_ipa::read_entry(&out.path, "SwiftSupport/iphoneos/libswiftCompatibilitySpan.dylib").unwrap();
    assert_eq!(bytes, fake_macho());
}

#[cfg(unix)]
#[test]
fn symlinks_keep_their_target() {
    let (_guard, root) = tmp();
    let app = make_app(&root);
    std::os::unix::fs::symlink("Info.plist", app.join("Link.plist")).unwrap();
    let out = russet_ipa::write(&app, &root.join("l.ipa"), opts(IpaMode::Store)).unwrap();
    let listing = russet_ipa::read_listing(&out.path).unwrap();
    let link = listing.iter().find(|e| e.name == "Payload/Probe.app/Link.plist").unwrap();
    assert_eq!(link.unix_mode, 0o120777);
    assert_eq!(link.symlink_target.as_deref(), Some("Info.plist"));
    assert_eq!(link.compression, Compression::Stored);
}

#[test]
fn non_app_inputs_are_refused() {
    let (_guard, root) = tmp();
    std::fs::create_dir_all(root.join("NotAnApp")).unwrap();
    assert!(russet_ipa::write(&root.join("NotAnApp"), &root.join("x.ipa"), opts(IpaMode::Dev)).is_err());
    assert!(!root.join("x.ipa").exists());
}

/// Cross-check with Info-ZIP's `unzip -Z` when it is installed (test-only; Russet never runs it).
#[test]
fn unzip_agrees_on_modes() {
    let Ok(out) = std::process::Command::new("unzip").arg("-v").output() else {
        eprintln!("skipped: unzip is not installed");
        return;
    };
    if !out.status.success() {
        return;
    }
    let (_guard, root) = tmp();
    let app = make_app(&root);
    let ipa = russet_ipa::write(&app, &root.join("z.ipa"), opts(IpaMode::Dev)).unwrap();
    let z = std::process::Command::new("unzip").arg("-Z").arg(ipa.path.as_str()).output().unwrap();
    let text = String::from_utf8_lossy(&z.stdout);
    let line = |name: &str| text.lines().find(|l| l.ends_with(name)).unwrap_or_default().to_owned();
    assert!(line("Payload/Probe.app/Probe").starts_with("-rwxr-xr-x"), "{text}");
    assert!(line("Payload/Probe.app/Info.plist").starts_with("-rw-r--r--"), "{text}");
    assert!(line("Payload/Probe.app/").starts_with("drwxr-xr-x"), "{text}");
}
