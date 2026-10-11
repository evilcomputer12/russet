// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! End-to-end signing of a synthetic app with the synthetic PKI: pre-sign fixes, inside-out signing through
//! apple-codesign, verification, tamper detection, IPA input and the lints (SIGN §8, §18).

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use common::*;
use plist::Value;
use russet_core::error::codes;
use russet_sign::testpki::{ProfileBuilder, TestIdentity, development_identity};
use russet_sign::{
    AppBundle, BundleKind, BundleSigningInput, CodeItem, Distribution, LintProfile, SignOptions, SignOutput,
    SignRequest, VerifyOptions,
};

const UDID: &str = "00008101-000A1B2C3D4E5F60";

struct Setup {
    _dir: tempfile::TempDir,
    root: PathBuf,
    app: PathBuf,
    id: TestIdentity,
    app_profile: PathBuf,
    widget_profile: PathBuf,
    entitlements: PathBuf,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let app = build_app(&root);
    let id = development_identity(TEAM);
    let app_profile = root.join("app.mobileprovision");
    std::fs::write(
        &app_profile,
        ProfileBuilder::development(TEAM, APP_ID).certificate(&id.cert_der).device(UDID).build(),
    )
    .unwrap();
    let widget_profile = root.join("widget.mobileprovision");
    std::fs::write(
        &widget_profile,
        ProfileBuilder::development(TEAM, "dev.russet.test.*").certificate(&id.cert_der).device(UDID).build(),
    )
    .unwrap();
    let entitlements = root.join("Probe.entitlements");
    write_plist(
        &entitlements,
        &[(
            "keychain-access-groups",
            Value::Array(vec![Value::String("$(AppIdentifierPrefix)$(PRODUCT_BUNDLE_IDENTIFIER)".into())]),
        )],
    );
    Setup { _dir: dir, root, app, id, app_profile, widget_profile, entitlements }
}

fn inputs(s: &Setup) -> BTreeMap<String, BundleSigningInput> {
    let mut m = BTreeMap::new();
    m.insert(
        String::new(),
        BundleSigningInput {
            profile: Some(s.app_profile.clone()),
            entitlements: Some(s.entitlements.clone()),
            get_task_allow: Some(true),
            ..Default::default()
        },
    );
    m.insert(
        "PlugIns/Widget.appex".into(),
        BundleSigningInput { profile: Some(s.widget_profile.clone()), ..Default::default() },
    );
    m
}

fn sign_to(
    s: &Setup,
    per_bundle: BTreeMap<String, BundleSigningInput>,
    out: &Path,
) -> russet_core::Result<russet_sign::SignReport> {
    let identity = s.id.load();
    russet_sign::sign(SignRequest {
        bundle: &s.app,
        identity: &identity,
        per_bundle,
        options: SignOptions { output: SignOutput::CopyTo(out.to_path_buf()), ..Default::default() },
    })
}

#[test]
fn walks_the_bundle_inside_out() {
    let s = setup();
    let app = AppBundle::open(&s.app).unwrap();
    assert_eq!(app.kind, BundleKind::App);
    assert_eq!(app.signing_order(), vec!["PlugIns/Widget.appex", "Frameworks/Kit.framework", ""]);
    assert_eq!(app.identifier(), Some(APP_ID));
    let ids: Vec<(String, String)> = app.bundle_ids();
    assert!(ids.contains(&("PlugIns/Widget.appex".into(), WIDGET_ID.into())));
    assert_eq!(app.macho_files().len(), 4);
    assert!(app.code.iter().any(|c| matches!(c, CodeItem::MainExecutable(p) if p == Path::new("Probe"))));
    assert!(
        app.code.iter().any(|c| matches!(c, CodeItem::Dylib(p) if p == Path::new("Frameworks/libswiftFixture.dylib")))
    );
    let kit = app.find("Frameworks/Kit.framework").unwrap();
    assert_eq!(kit.kind, BundleKind::Framework);
    assert!(matches!(kit.code.as_slice(), [CodeItem::Framework(_)]));
    assert_eq!(app.find("PlugIns/Widget.appex").unwrap().kind, BundleKind::AppExtension);
    assert_eq!(AppBundle::open(&s.root.join("missing.app")).unwrap_err().code(), codes::sign::BUNDLE_INVALID);
}

#[test]
fn signs_verifies_and_inspects_an_app() {
    let s = setup();
    let out = s.root.join("out/Probe.app");
    let report = sign_to(&s, inputs(&s), &out).unwrap();
    assert_eq!(report.output, out);

    // The build product is untouched.
    let src_exe = russet_macho::MachO::open(&s.app.join("Probe")).unwrap();
    assert_eq!(src_exe.string_pool_alignment(), 4);
    assert!(!s.app.join("embedded.mobileprovision").exists());
    assert!(s.app.join(".DS_Store").exists());

    // Pre-sign fixes on the copy.
    let fixed: Vec<(&str, &str)> = report.fixes_applied.iter().map(|f| (f.fix.as_str(), f.path.as_str())).collect();
    assert!(fixed.contains(&("align-string-pool", "Probe")), "{fixed:?}");
    assert!(fixed.contains(&("clean", ".DS_Store")), "{fixed:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert!(fixed.contains(&("chmod", "Probe")), "{fixed:?}");
        assert_eq!(std::fs::metadata(out.join("Probe")).unwrap().permissions().mode() & 0o777, 0o755);
    }

    // Two app-like bundles, deepest first.
    let rels: Vec<&str> = report.bundles.iter().map(|b| b.rel_path.as_str()).collect();
    assert_eq!(rels, vec!["PlugIns/Widget.appex", ""]);
    let app_b = &report.bundles[1];
    assert_eq!(app_b.bundle_id, APP_ID);
    assert_eq!(app_b.cdhash.len(), 40);
    assert_eq!(app_b.team_id, TEAM);
    assert!(app_b.entitlements_xml.contains("ABCDE12345.dev.russet.test.probe"));
    assert!(out.join("embedded.mobileprovision").is_file());
    assert!(out.join("PlugIns/Widget.appex/embedded.mobileprovision").is_file());
    assert!(!out.join("Frameworks/Kit.framework/embedded.mobileprovision").exists());

    let v = russet_sign::verify(&out, &VerifyOptions { device: Some(UDID.into()), ..Default::default() }).unwrap();
    assert!(v.ok, "{:#?}", v.findings);
    assert!(v.bundles.iter().all(|b| b.signed));
    assert_eq!(
        v.bundles.iter().find(|b| b.rel_path.is_empty()).unwrap().cdhash.as_deref(),
        Some(app_b.cdhash.as_str())
    );

    let info = russet_sign::inspect(&out).unwrap();
    let probe = info.iter().find(|i| i.path == "Probe").unwrap();
    assert_eq!(probe.bundle_id.as_deref(), Some(APP_ID));
    assert_eq!(probe.identifier.as_deref(), Some(APP_ID));
    assert_eq!(probe.team_id.as_deref(), Some("*******345"));
    assert!(probe.has_cms && probe.has_der_entitlements && probe.string_pool_aligned);
    assert_eq!(probe.get_task_allow, Some(true));
    let cd = &probe.code_directories[0];
    assert_eq!(cd.exec_seg_flags_raw, Some(0x11), "MAIN_BINARY | ALLOW_UNSIGNED, as on the M1 Hello [V11]");
    assert!(u32::from_str_radix(cd.version.trim_start_matches("0x"), 16).unwrap() >= 0x20400);
    assert!(probe.code_directories.iter().any(|c| c.hash_type == "sha256"));
    assert!(!cd.flags.contains(&"adhoc".to_owned()) && !cd.flags.contains(&"linker-signed".to_owned()));
    let kit = info.iter().find(|i| i.path == "Frameworks/Kit.framework/Kit").unwrap();
    assert!(kit.signed && kit.entitlement_keys.is_empty());
    let widget = info.iter().find(|i| i.path == "PlugIns/Widget.appex/Widget").unwrap();
    assert!(widget.code_directories[0].exec_seg_flags.contains(&"main-binary".to_owned()));
    let swift = info.iter().find(|i| i.path == "Frameworks/libswiftFixture.dylib").unwrap();
    assert!(swift.signed);
    assert_eq!(info.len(), 4);

    // Re-signing the signed output in place works and still verifies.
    let identity = s.id.load();
    let mut per = inputs(&s);
    per.get_mut("").unwrap().get_task_allow = Some(false);
    let again = russet_sign::sign(SignRequest {
        bundle: &out,
        identity: &identity,
        per_bundle: per,
        options: SignOptions::default(),
    })
    .unwrap();
    assert_ne!(again.bundles[1].cdhash, app_b.cdhash);
    let probe = russet_sign::inspect(&out).unwrap().into_iter().find(|i| i.path == "Probe").unwrap();
    assert_eq!(probe.get_task_allow, Some(false));
    assert_eq!(probe.code_directories[0].exec_seg_flags_raw, Some(0x1), "no ALLOW_UNSIGNED without get-task-allow");
    assert!(russet_sign::verify(&out, &VerifyOptions::default()).unwrap().ok);
}

fn codes_of(v: &russet_sign::VerifyReport) -> Vec<(String, String)> {
    v.findings.iter().filter(|f| f.is_error()).map(|f| (f.code.clone(), f.path.clone().unwrap_or_default())).collect()
}

#[test]
fn tampering_breaks_verification() {
    let s = setup();
    let out = s.root.join("signed/Probe.app");
    sign_to(&s, inputs(&s), &out).unwrap();
    assert!(russet_sign::verify(&out, &VerifyOptions::default()).unwrap().ok);

    std::fs::write(out.join("readme.txt"), b"changed\n").unwrap();
    let v = russet_sign::verify(&out, &VerifyOptions::default()).unwrap();
    assert!(!v.ok);
    assert!(codes_of(&v).contains(&("L-SIG-006".into(), "readme.txt".into())), "{:?}", codes_of(&v));

    std::fs::write(out.join("readme.txt"), b"a resource\n").unwrap();
    assert!(russet_sign::verify(&out, &VerifyOptions::default()).unwrap().ok, "restoring the file restores the seal");

    std::fs::write(out.join("added.txt"), b"new").unwrap();
    let v = russet_sign::verify(&out, &VerifyOptions::default()).unwrap();
    assert!(codes_of(&v).contains(&("L-SIG-006".into(), "added.txt".into())));
    std::fs::remove_file(out.join("added.txt")).unwrap();

    let mut exe = std::fs::read(out.join("PlugIns/Widget.appex/Widget")).unwrap();
    exe[0x1000] ^= 0xff;
    std::fs::write(out.join("PlugIns/Widget.appex/Widget"), &exe).unwrap();
    let v = russet_sign::verify(&out, &VerifyOptions::default()).unwrap();
    let c = codes_of(&v);
    assert!(c.iter().any(|(code, path)| code == "L-SIG-006" && path == "PlugIns/Widget.appex/Widget"), "{c:?}");
    // The app's seal of the widget (its CDHash) still matches: only the code pages changed.
    exe[0x1000] ^= 0xff;
    std::fs::write(out.join("PlugIns/Widget.appex/Widget"), &exe).unwrap();

    let info = out.join("Info.plist");
    let mut d = plist::Value::from_file(&info).unwrap().into_dictionary().unwrap();
    d.insert("CFBundleVersion".into(), Value::String("2".into()));
    Value::Dictionary(d).to_file_xml(&info).unwrap();
    let v = russet_sign::verify(&out, &VerifyOptions::default()).unwrap();
    assert!(codes_of(&v).iter().any(|(code, path)| code == "L-SIG-006" && path == "Probe"));

    let v = russet_sign::verify(
        &out,
        &VerifyOptions { device: Some("00008101-FFFFFFFFFFFFFFFF".into()), ..Default::default() },
    )
    .unwrap();
    assert!(codes_of(&v).iter().any(|(code, _)| code == "L-SIG-007"));
}

#[test]
fn refuses_missing_or_mismatched_profiles() {
    let s = setup();
    let out = s.root.join("o/Probe.app");

    let mut per = inputs(&s);
    per.remove("PlugIns/Widget.appex");
    assert_eq!(sign_to(&s, per, &out).unwrap_err().code(), codes::sign::PROFILE_MISMATCH);

    let other = s.root.join("other.mobileprovision");
    std::fs::write(&other, ProfileBuilder::development(TEAM, "com.other.app").certificate(&s.id.cert_der).build())
        .unwrap();
    let mut per = inputs(&s);
    per.get_mut("").unwrap().profile = Some(other);
    assert_eq!(sign_to(&s, per, &out).unwrap_err().code(), codes::sign::PROFILE_MISMATCH);

    let nocert = s.root.join("nocert.mobileprovision");
    std::fs::write(&nocert, ProfileBuilder::development(TEAM, APP_ID).build()).unwrap();
    let mut per = inputs(&s);
    per.get_mut("").unwrap().profile = Some(nocert);
    assert_eq!(sign_to(&s, per, &out).unwrap_err().code(), codes::sign::PROFILE_MISMATCH);

    let expired = s.root.join("expired.mobileprovision");
    std::fs::write(&expired, ProfileBuilder::development(TEAM, APP_ID).certificate(&s.id.cert_der).expired().build())
        .unwrap();
    let mut per = inputs(&s);
    per.get_mut("").unwrap().profile = Some(expired);
    assert_eq!(sign_to(&s, per, &out).unwrap_err().code(), codes::sign::PROFILE_MISMATCH);

    let store = s.root.join("store.mobileprovision");
    std::fs::write(&store, ProfileBuilder::app_store(TEAM, APP_ID).certificate(&s.id.cert_der).build()).unwrap();
    let mut per = inputs(&s);
    per.get_mut("").unwrap().profile = Some(store);
    per.get_mut("").unwrap().get_task_allow = None;
    assert_eq!(sign_to(&s, per, &out).unwrap_err().code(), codes::sign::PROFILE_MISMATCH, "development distribution");

    let push = s.root.join("push.entitlements");
    write_plist(&push, &[("aps-environment", Value::String("development".into()))]);
    let mut per = inputs(&s);
    per.get_mut("").unwrap().entitlements = Some(push);
    let e = sign_to(&s, per, &out).unwrap_err();
    assert_eq!(e.code(), codes::sign::ENTITLEMENT_NOT_IN_PROFILE);

    let mut per = inputs(&s);
    per.insert("PlugIns/Nope.appex".into(), BundleSigningInput::default());
    assert_eq!(sign_to(&s, per, &out).unwrap_err().code(), codes::sign::BUNDLE_INVALID);

    // A previous output is replaced, anything else is not.
    std::fs::create_dir_all(s.root.join("notabundle")).unwrap();
    std::fs::write(s.root.join("notabundle/file"), b"keep me").unwrap();
    let e = sign_to(&s, inputs(&s), &s.root.join("notabundle")).unwrap_err();
    assert_eq!(e.code(), codes::core::INVALID_ARGUMENT);
    assert!(s.root.join("notabundle/file").exists());
    sign_to(&s, inputs(&s), &out).unwrap();
    sign_to(&s, inputs(&s), &out).unwrap();
}

fn zip_app(app: &Path, ipa: &Path, swift_support: bool) {
    let f = std::fs::File::create(ipa).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    let base = app.parent().unwrap();
    let mut files = Vec::new();
    let mut stack = vec![app.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() { stack.push(p) } else { files.push(p) }
        }
    }
    files.sort();
    for p in files {
        let name = format!("Payload/{}", p.strip_prefix(base).unwrap().to_string_lossy());
        z.start_file(name, opts.unix_permissions(0o755)).unwrap();
        z.write_all(&std::fs::read(&p).unwrap()).unwrap();
    }
    if swift_support {
        z.start_file("SwiftSupport/iphoneos/libswiftFixture.dylib", opts).unwrap();
        z.write_all(b"original").unwrap();
    }
    z.finish().unwrap();
}

#[test]
fn reads_ipas_and_lints_for_the_store() {
    let s = setup();
    let out = s.root.join("ipa/Probe.app");
    sign_to(&s, inputs(&s), &out).unwrap();
    let ipa = s.root.join("Probe.ipa");
    zip_app(&out, &ipa, false);
    assert!(russet_sign::verify(&ipa, &VerifyOptions::default()).unwrap().ok);
    assert_eq!(russet_sign::inspect(&ipa).unwrap().len(), 4);

    let findings = russet_sign::lint(&ipa, LintProfile::AppStore).unwrap();
    let errors: Vec<&str> = findings.iter().filter(|f| f.is_error()).map(|f| f.code.as_str()).collect();
    assert!(errors.contains(&"L-BND-010"), "{errors:?}");
    assert!(errors.contains(&"L-BND-007"), "{errors:?}");
    assert!(!errors.contains(&"L-SIG-001"));
    let with_support = s.root.join("Store.ipa");
    zip_app(&out, &with_support, true);
    let findings = russet_sign::lint(&with_support, LintProfile::AppStore).unwrap();
    assert!(!findings.iter().any(|f| f.code == "L-BND-010" && f.is_error()));

    let device = russet_sign::lint(&ipa, LintProfile::DeviceInstall).unwrap();
    assert!(!device.iter().any(|f| f.is_error()), "{device:#?}");

    // Unsafe archives are refused.
    let evil = s.root.join("evil.ipa");
    let mut z = zip::ZipWriter::new(std::fs::File::create(&evil).unwrap());
    z.start_file("Payload/../../escape.txt", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(b"x").unwrap();
    z.finish().unwrap();
    assert_eq!(russet_sign::inspect(&evil).unwrap_err().code(), codes::sign::BUNDLE_INVALID);
    assert!(!s.root.parent().unwrap().join("escape.txt").exists());
}

#[test]
fn lints_an_unsigned_build() {
    let s = setup();
    let findings = russet_sign::lint(&s.app, LintProfile::DeviceInstall).unwrap();
    assert!(findings.iter().any(|f| f.code == "L-SIG-000" && !f.is_error()));
    assert!(findings.iter().any(|f| f.code == "L-MACHO-001" && f.is_error() && f.path.as_deref() == Some("Probe")));
    assert!(!findings.iter().any(|f| f.code == "L-BND-009"), "the fixture declares a scene manifest");

    // The store profile adds the bundle rules.
    let store = russet_sign::lint(&s.app, LintProfile::AppStore).unwrap();
    assert!(store.iter().any(|f| f.code == "L-BND-007"));
    let d = Distribution::parse("app-store-connect").unwrap();
    assert_eq!(d.as_str(), "app-store-connect");
}
