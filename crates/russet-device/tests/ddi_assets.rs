// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! DDI directories (ARCHITECTURE §5.3 `ddi`) [D 82, D 88]: a synthetic DDI root, generated at test time in a
//! temporary directory and never committed, with Xcode-native payload names (`022-…-411.dmg`, `Firmware/…`), and a
//! bare `Restore` directory. Payload paths come from the cryptex build identity of the BuildManifest.

use std::path::Path;

use idevice::cryptexd::Cryptex1Assets;
use russet_core::error::codes;
use russet_device::ddi::{DdiOrigin, MountRecord, cryptex_install_argv, ddi_build, resolve_restore_dir, sha384_hex};

const IMAGE: &str = "022-00002-411.dmg";
const TRUSTCACHE: &str = "Firmware/022-00002-411.dmg.trustcache";
const INFO: &str = "Firmware/022-00002-411.dmg.cryptex1info.plist";
const VOLUME: &str = "Firmware/022-00002-411.dmg.root_hash";

fn dict(pairs: Vec<(&str, plist::Value)>) -> plist::Value {
    let mut d = plist::Dictionary::new();
    for (k, v) in pairs {
        d.insert(k.to_owned(), v);
    }
    plist::Value::Dictionary(d)
}

fn entry(path: &str) -> plist::Value {
    dict(vec![("Info", dict(vec![("Path", path.into())])), ("Digest", plist::Value::Data(vec![0; 48]))])
}

/// A BuildManifest with one personalized-DMG identity and the cryptex identity, as Xcode 27's carries them.
fn manifest(build: &str) -> plist::Value {
    let pdi = dict(vec![
        ("Info", dict(vec![("Variant", "Customer iOS Developer PDI".into())])),
        ("Manifest", dict(vec![("PersonalizedDMG", entry("022-00001-411.dmg"))])),
    ]);
    let cryptex = dict(vec![
        ("Info", dict(vec![("Variant", "iOS Customer Developer Disk Image Cryptex".into())])),
        ("Cryptex1,NonceDomain", plist::Value::Integer(4u64.into())),
        ("Cryptex1,SubType", plist::Value::Integer(1u64.into())),
        ("Cryptex1,Version", "27.9.999.1".into()),
        ("Cryptex1,PreauthorizationVersion", "27.9.999.1".into()),
        ("Cryptex1,UseProductClass", true.into()),
        (
            "Manifest",
            dict(vec![
                ("Cryptex1,GenericDmg", entry(IMAGE)),
                ("Cryptex1,GenericTrustCache", entry(TRUSTCACHE)),
                ("Cryptex1,CryptexInfoPlist", entry(INFO)),
                ("Cryptex1,GenericVolume", entry(VOLUME)),
            ]),
        ),
    ]);
    dict(vec![("ProductBuildVersion", build.into()), ("BuildIdentities", plist::Value::Array(vec![pdi, cryptex]))])
}

/// Writes a synthetic `Restore` directory (synthetic bytes, no Apple content).
fn write_restore(restore: &Path, build: &str) {
    std::fs::create_dir_all(restore.join("Firmware")).expect("mkdir");
    plist::to_file_xml(restore.join("BuildManifest.plist"), &manifest(build)).expect("manifest");
    std::fs::write(restore.join(IMAGE), b"russet synthetic cryptex image").expect("image");
    std::fs::write(restore.join("022-00001-411.dmg"), b"russet synthetic personalized image").expect("pdi");
    std::fs::write(restore.join(TRUSTCACHE), b"tc").expect("trustcache");
    std::fs::write(restore.join(INFO), b"<plist/>").expect("info");
    std::fs::write(restore.join(VOLUME), b"roothash").expect("volume");
}

#[tokio::test]
async fn a_synthetic_ddi_root_loads_from_its_restore_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("iOS_DDI");
    write_restore(&root.join("Restore"), "27Z999z");
    plist::to_file_xml(root.join("version.plist"), &dict(vec![("ProductBuildVersion", "27Z999y".into())]))
        .expect("version.plist");

    let restore = resolve_restore_dir(&root).expect("a DDI root");
    assert_eq!(restore, root.join("Restore"));
    // version.plist next to Restore/ wins over the manifest.
    assert_eq!(ddi_build(&root, &restore).as_deref(), Some("27Z999y"));

    let assets = Cryptex1Assets::load(&restore).await.expect("the cryptex identity's payloads");
    assert_eq!(assets.image, b"russet synthetic cryptex image");
    assert_eq!(assets.trustcache, b"tc");
    assert_eq!(assets.volumehash, b"roothash");
    assert_eq!(assets.nonce_domain().expect("nonce domain"), 4);
    let props = assets.cryptex1_properties().expect("cryptex1 properties");
    let argv = cryptex_install_argv(props, [1, 2, 3, 4, 5], 2, 1);
    assert_eq!(argv.get("persistence"), Some(&idevice::xpc::XPCObject::UInt64(2)));
    assert_eq!(argv.get("nonce-persistence"), Some(&idevice::xpc::XPCObject::UInt64(1)));

    // What a mount from this directory would record [D 89].
    let record = MountRecord {
        source: DdiOrigin::Dir,
        path: Some(root.clone()),
        ddi_build: ddi_build(&root, &restore).unwrap_or_default(),
        cryptex_version: Some("27.9.999.1".into()),
        image_sha384: sha384_hex(&assets.image),
        mounted_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    assert_eq!(record.image_sha384.len(), 96);
}

#[tokio::test]
async fn a_bare_restore_directory_also_works() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let restore = tmp.path().join("Restore");
    write_restore(&restore, "27Z999z");
    assert_eq!(resolve_restore_dir(&restore).expect("a Restore dir"), restore);
    // Without version.plist the manifest's build is used.
    assert_eq!(ddi_build(&restore, &restore).as_deref(), Some("27Z999z"));
    let assets = Cryptex1Assets::load(&restore).await.expect("payloads");
    assert_eq!(assets.info, b"<plist/>");
}

#[test]
fn a_directory_without_a_manifest_is_no_ddi_source() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let err = resolve_restore_dir(tmp.path()).expect_err("no manifest");
    assert_eq!(err.code(), codes::device::DDI_SOURCE_MISSING);
}
