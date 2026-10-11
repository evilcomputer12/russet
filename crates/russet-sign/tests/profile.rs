// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Provisioning-profile parsing on synthetic CMS profiles from the test PKI (PLAN W6).

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use russet_core::error::codes;
use russet_sign::testpki::{ProfileBuilder, development_identity};
use russet_sign::{ProfileKind, ProvisioningProfile};

#[test]
fn parses_a_development_profile() {
    let id = development_identity("ABCDE12345");
    let raw = ProfileBuilder::development("ABCDE12345", "com.example.hello")
        .uuid("11111111-2222-4333-8444-555555555555")
        .name("XTL Example")
        .certificate(&id.cert_der)
        .device("00008101-000A1B2C3D4E5F60")
        .build();
    let p = ProvisioningProfile::parse(&raw).unwrap();
    assert_eq!(p.uuid, "11111111-2222-4333-8444-555555555555");
    assert_eq!(p.name, "XTL Example");
    assert_eq!(p.team_id, "ABCDE12345");
    assert_eq!(p.app_id_prefix, "ABCDE12345");
    assert_eq!(p.application_identifier, "ABCDE12345.com.example.hello");
    assert_eq!(p.bundle_id_pattern(), "com.example.hello");
    assert!(!p.is_wildcard());
    assert_eq!(p.kind, ProfileKind::Development);
    assert_eq!(p.platforms, vec!["iOS", "xrOS", "visionOS"]);
    assert_eq!(p.ppq_check, Some(true));
    assert!(p.has_der_profile);
    assert!(p.get_task_allow());
    assert_eq!(p.raw, raw);
    assert!(p.has_certificate(&id.cert_der));
    assert!(!p.has_certificate(&development_identity("ABCDE12345").cert_der));
    assert!(p.has_device("00008101-000a1b2c3d4e5f60"), "UDIDs compare case-insensitively");
    assert!(!p.has_device("00008101-FFFFFFFFFFFFFFFF"));
    assert!(p.covers("com.example.hello", "ABCDE12345"));
    assert!(!p.covers("com.example.hello", "ZZZZZ99999"));
    assert!(!p.covers("com.example.other", "ABCDE12345"));
    assert!(!p.expires_within(Duration::from_secs(24 * 3600)));
    assert!(p.expires_within(Duration::from_secs(400 * 24 * 3600)));
}

#[test]
fn derives_the_kind_and_matches_wildcards() {
    let store = ProvisioningProfile::parse(&ProfileBuilder::app_store("ABCDE12345", "com.example.*").build()).unwrap();
    assert_eq!(store.kind, ProfileKind::AppStore);
    assert!(store.is_wildcard());
    assert!(store.covers("com.example.anything", "ABCDE12345"));
    assert!(!store.covers("org.other", "ABCDE12345"));
    assert!(!store.has_device("00008101-000A1B2C3D4E5F60"));

    let adhoc = ProvisioningProfile::parse(&ProfileBuilder::ad_hoc("ABCDE12345", "*").build()).unwrap();
    assert_eq!(adhoc.kind, ProfileKind::AdHoc);
    assert!(adhoc.covers("whatever.bundle", "ABCDE12345"));

    let mut inhouse = ProfileBuilder::app_store("ABCDE12345", "*");
    inhouse.provisions_all_devices = true;
    let inhouse = ProvisioningProfile::parse(&inhouse.build()).unwrap();
    assert_eq!(inhouse.kind, ProfileKind::InHouse);
    assert!(inhouse.has_device("any"));

    let expired =
        ProvisioningProfile::parse(&ProfileBuilder::development("ABCDE12345", "*").expired().build()).unwrap();
    assert!(expired.expires_within(Duration::ZERO));
}

#[test]
fn rejects_what_is_not_a_profile() {
    for bad in [&b""[..], b"not cms at all", &[0x30, 0x82, 0x00, 0x10]] {
        let e = ProvisioningProfile::parse(bad).unwrap_err();
        assert_eq!(e.code(), codes::sign::PROFILE_INVALID);
        assert!(e.hint().is_some());
    }
    // CMS whose content is a plist that is not a profile.
    let mut pb = ProfileBuilder::development("ABCDE12345", "*");
    pb.entitlements.clear();
    let e = ProvisioningProfile::parse(&pb.build()).unwrap_err();
    assert_eq!(e.code(), codes::sign::PROFILE_INVALID);
}
