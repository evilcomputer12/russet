// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A synthetic app tree for signer tests, built at test time from russet-macho's fixture builder (ARCHITECTURE
//! §19: no Mach-O fixture is committed).

#![allow(dead_code, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use plist::{Dictionary, Value};
use russet_macho::fixture::MachOBuilder;

pub const TEAM: &str = "ABCDE12345";
pub const APP_ID: &str = "dev.russet.test.probe";
pub const WIDGET_ID: &str = "dev.russet.test.probe.widget";

pub fn write_plist(path: &Path, entries: &[(&str, Value)]) {
    let mut d = Dictionary::new();
    for (k, v) in entries {
        d.insert((*k).into(), v.clone());
    }
    Value::Dictionary(d).to_file_xml(path).unwrap();
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
pub fn set_mode(_path: &Path, _mode: u32) {}

/// `Probe.app` with a framework, a loose Swift dylib and a widget extension. The app executable is linker-signed,
/// has a misaligned string pool (sdk 27.0) and mode 0644, like an unpacked lld build.
pub fn build_app(dir: &Path) -> PathBuf {
    let app = dir.join("Probe.app");
    std::fs::create_dir_all(app.join("Frameworks/Kit.framework")).unwrap();
    std::fs::create_dir_all(app.join("PlugIns/Widget.appex")).unwrap();
    std::fs::create_dir_all(app.join("Base.lproj")).unwrap();
    write_plist(
        &app.join("Info.plist"),
        &[
            ("CFBundleIdentifier", s(APP_ID)),
            ("CFBundleExecutable", s("Probe")),
            ("CFBundleName", s("Probe")),
            ("CFBundlePackageType", s("APPL")),
            ("CFBundleShortVersionString", s("1.0")),
            ("CFBundleVersion", s("1")),
            ("MinimumOSVersion", s("17.0")),
            ("UIDeviceFamily", Value::Array(vec![Value::Integer(1.into())])),
            ("DTPlatformName", s("iphoneos")),
            ("UIApplicationSceneManifest", Value::Dictionary(Dictionary::new())),
        ],
    );
    let exe = app.join("Probe");
    std::fs::write(
        &exe,
        MachOBuilder::executable()
            .rpath("@executable_path/Frameworks")
            .dylib_dependency("@rpath/Kit.framework/Kit", false)
            .encryption_info()
            .misaligned_string_pool()
            .linker_signed("Probe")
            .build(),
    )
    .unwrap();
    set_mode(&exe, 0o644);
    std::fs::write(app.join("PkgInfo"), b"APPL????").unwrap();
    std::fs::write(app.join("Base.lproj/Main.strings"), b"\"hello\" = \"hello\";\n").unwrap();
    std::fs::write(app.join("readme.txt"), b"a resource\n").unwrap();
    std::fs::write(app.join(".DS_Store"), b"finder litter").unwrap();

    let fw = app.join("Frameworks/Kit.framework");
    write_plist(
        &fw.join("Info.plist"),
        &[
            ("CFBundleIdentifier", s("dev.russet.test.Kit")),
            ("CFBundleExecutable", s("Kit")),
            ("CFBundlePackageType", s("FMWK")),
            ("CFBundleShortVersionString", s("1.0")),
            ("CFBundleVersion", s("1")),
        ],
    );
    std::fs::write(fw.join("Kit"), MachOBuilder::dylib("@rpath/Kit.framework/Kit").build()).unwrap();
    std::fs::write(
        app.join("Frameworks/libswiftFixture.dylib"),
        MachOBuilder::dylib("@rpath/libswiftFixture.dylib").build(),
    )
    .unwrap();

    let ext = app.join("PlugIns/Widget.appex");
    let mut ns_ext = Dictionary::new();
    ns_ext.insert("NSExtensionPointIdentifier".into(), s("com.apple.widgetkit-extension"));
    write_plist(
        &ext.join("Info.plist"),
        &[
            ("CFBundleIdentifier", s(WIDGET_ID)),
            ("CFBundleExecutable", s("Widget")),
            ("CFBundlePackageType", s("XPC!")),
            ("CFBundleShortVersionString", s("1.0")),
            ("CFBundleVersion", s("1")),
            ("MinimumOSVersion", s("17.0")),
            ("NSExtension", Value::Dictionary(ns_ext)),
        ],
    );
    std::fs::write(ext.join("Widget"), MachOBuilder::executable().uuid([0x77; 16]).build()).unwrap();
    app
}
