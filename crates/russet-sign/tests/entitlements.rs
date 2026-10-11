// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The entitlement merge of SIGN §8.4, table-driven (PLAN W6: at least 40 cases).

#![allow(clippy::unwrap_used)]

use plist::{Dictionary, Value};
use russet_core::error::codes;
use russet_sign::testpki::ProfileBuilder;
use russet_sign::{Distribution, EntitlementsError, EntitlementsResolver, MergePolicy, ProvisioningProfile};

const T: &str = "ABCDE12345";

fn s(v: &str) -> Value {
    Value::String(v.into())
}
fn b(v: bool) -> Value {
    Value::Boolean(v)
}
fn arr(v: &[&str]) -> Value {
    Value::Array(v.iter().map(|x| s(x)).collect())
}
fn dict(v: &[(&str, Value)]) -> Value {
    let mut d = Dictionary::new();
    for (k, x) in v {
        d.insert((*k).into(), x.clone());
    }
    Value::Dictionary(d)
}

#[derive(Clone, Copy)]
enum Kind {
    Dev,
    Store,
    AdHoc,
}

enum Expect {
    /// Keys and their signed values; `None` asserts the key is absent.
    Ok(Vec<(&'static str, Option<Value>)>),
    /// The error variant, by name.
    Err(&'static str),
}

struct Case {
    name: &'static str,
    kind: Kind,
    pattern: &'static str,
    profile_set: Vec<(&'static str, Value)>,
    profile_remove: Vec<&'static str>,
    bundle: &'static str,
    requested: Vec<(&'static str, Value)>,
    dist: Distribution,
    policy: MergePolicy,
    expect: Expect,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            name: "",
            kind: Kind::Dev,
            pattern: "com.example.app",
            profile_set: Vec::new(),
            profile_remove: Vec::new(),
            bundle: "com.example.app",
            requested: Vec::new(),
            dist: Distribution::Development,
            policy: MergePolicy::Least,
            expect: Expect::Ok(Vec::new()),
        }
    }
}

fn app_id(bundle: &str) -> Value {
    s(&format!("{T}.{bundle}"))
}

fn cases() -> Vec<Case> {
    use Expect::{Err, Ok};
    vec![
        Case {
            name: "empty request signs the required keys",
            expect: Ok(vec![
                ("application-identifier", Some(app_id("com.example.app"))),
                ("com.apple.developer.team-identifier", Some(s(T))),
                ("get-task-allow", Some(b(true))),
                ("keychain-access-groups", Some(arr(&["ABCDE12345.com.example.app"]))),
                ("beta-reports-active", None),
            ]),
            ..Default::default()
        },
        Case {
            name: "get-task-allow false is allowed under a development profile",
            requested: vec![("get-task-allow", b(false))],
            expect: Ok(vec![("get-task-allow", Some(b(false)))]),
            ..Default::default()
        },
        Case {
            name: "get-task-allow true is refused under an App Store profile",
            kind: Kind::Store,
            dist: Distribution::AppStore,
            requested: vec![("get-task-allow", b(true))],
            expect: Err("ValueNotAllowed"),
            ..Default::default()
        },
        Case {
            name: "App Store distribution signs beta-reports-active",
            kind: Kind::Store,
            dist: Distribution::AppStore,
            expect: Ok(vec![("beta-reports-active", Some(b(true))), ("get-task-allow", Some(b(false)))]),
            ..Default::default()
        },
        Case {
            name: "development distribution never signs beta-reports-active",
            profile_set: vec![("beta-reports-active", b(true))],
            expect: Ok(vec![("beta-reports-active", None)]),
            ..Default::default()
        },
        Case {
            name: "beta-reports-active requested without the profile granting it",
            requested: vec![("beta-reports-active", b(true))],
            expect: Err("ValueNotAllowed"),
            ..Default::default()
        },
        Case {
            name: "ad hoc profiles sign get-task-allow false",
            kind: Kind::AdHoc,
            dist: Distribution::AdHoc,
            expect: Ok(vec![("get-task-allow", Some(b(false))), ("beta-reports-active", None)]),
            ..Default::default()
        },
        Case {
            name: "$(AppIdentifierPrefix) expands in keychain groups",
            requested: vec![("keychain-access-groups", arr(&["$(AppIdentifierPrefix)com.example.app"]))],
            expect: Ok(vec![("keychain-access-groups", Some(arr(&["ABCDE12345.com.example.app"])))]),
            ..Default::default()
        },
        Case {
            name: "$(TeamIdentifierPrefix) expands",
            requested: vec![("keychain-access-groups", arr(&["$(TeamIdentifierPrefix)shared"]))],
            expect: Ok(vec![("keychain-access-groups", Some(arr(&["ABCDE12345.shared"])))]),
            ..Default::default()
        },
        Case {
            name: "$(PRODUCT_BUNDLE_IDENTIFIER) expands",
            requested: vec![("keychain-access-groups", arr(&["ABCDE12345.$(PRODUCT_BUNDLE_IDENTIFIER)"]))],
            expect: Ok(vec![("keychain-access-groups", Some(arr(&["ABCDE12345.com.example.app"])))]),
            ..Default::default()
        },
        Case {
            name: "${PRODUCT_BUNDLE_IDENTIFIER} brace form expands",
            requested: vec![("keychain-access-groups", arr(&["ABCDE12345.${PRODUCT_BUNDLE_IDENTIFIER}"]))],
            expect: Ok(vec![("keychain-access-groups", Some(arr(&["ABCDE12345.com.example.app"])))]),
            ..Default::default()
        },
        Case {
            name: "$(CFBundleIdentifier) expands",
            requested: vec![("keychain-access-groups", arr(&["$(AppIdentifierPrefix)$(CFBundleIdentifier)"]))],
            expect: Ok(vec![("keychain-access-groups", Some(arr(&["ABCDE12345.com.example.app"])))]),
            ..Default::default()
        },
        Case {
            name: "a :rfc1034identifier modifier is accepted",
            requested: vec![(
                "keychain-access-groups",
                arr(&["ABCDE12345.$(PRODUCT_BUNDLE_IDENTIFIER:rfc1034identifier)"]),
            )],
            expect: Ok(vec![("keychain-access-groups", Some(arr(&["ABCDE12345.com.example.app"])))]),
            ..Default::default()
        },
        Case {
            name: "an unknown variable is refused",
            requested: vec![("keychain-access-groups", arr(&["$(SOMETHING_ELSE).x"]))],
            expect: Err("UnresolvedVariable"),
            ..Default::default()
        },
        Case {
            name: "an unterminated variable is refused",
            requested: vec![("keychain-access-groups", arr(&["$(AppIdentifierPrefix"]))],
            expect: Err("UnresolvedVariable"),
            ..Default::default()
        },
        Case {
            name: "an exact App ID does not cover another bundle",
            bundle: "com.other.app",
            expect: Err("AppIdMismatch"),
            ..Default::default()
        },
        Case {
            name: "a full wildcard App ID covers any bundle",
            pattern: "*",
            bundle: "org.anything.at.all",
            expect: Ok(vec![("application-identifier", Some(app_id("org.anything.at.all")))]),
            ..Default::default()
        },
        Case {
            name: "a prefix wildcard covers a matching bundle",
            pattern: "com.example.*",
            bundle: "com.example.app2",
            expect: Ok(vec![("application-identifier", Some(app_id("com.example.app2")))]),
            ..Default::default()
        },
        Case {
            name: "a prefix wildcard does not cover another prefix",
            pattern: "com.example.*",
            bundle: "com.other.app",
            expect: Err("AppIdMismatch"),
            ..Default::default()
        },
        Case {
            name: "a matching requested application-identifier is fine",
            requested: vec![("application-identifier", s("$(AppIdentifierPrefix)$(PRODUCT_BUNDLE_IDENTIFIER)"))],
            expect: Ok(vec![("application-identifier", Some(app_id("com.example.app")))]),
            ..Default::default()
        },
        Case {
            name: "a different requested application-identifier is refused",
            requested: vec![("application-identifier", s("ABCDE12345.com.example.other"))],
            expect: Err("ValueNotAllowed"),
            ..Default::default()
        },
        Case {
            name: "aps-environment is signed with the profile's value",
            profile_set: vec![("aps-environment", s("production"))],
            requested: vec![("aps-environment", s("development"))],
            expect: Ok(vec![("aps-environment", Some(s("production")))]),
            ..Default::default()
        },
        Case {
            name: "aps-environment without push in the profile",
            requested: vec![("aps-environment", s("development"))],
            expect: Err("NotInProfile"),
            ..Default::default()
        },
        Case {
            name: "associated domains under a `*` grant",
            profile_set: vec![("com.apple.developer.associated-domains", s("*"))],
            requested: vec![(
                "com.apple.developer.associated-domains",
                arr(&["applinks:example.com", "webcredentials:example.com"]),
            )],
            expect: Ok(vec![(
                "com.apple.developer.associated-domains",
                Some(arr(&["applinks:example.com", "webcredentials:example.com"])),
            )]),
            ..Default::default()
        },
        Case {
            name: "associated domains under a [`*`] grant",
            profile_set: vec![("com.apple.developer.associated-domains", arr(&["*"]))],
            requested: vec![("com.apple.developer.associated-domains", arr(&["applinks:example.com"]))],
            expect: Ok(vec![("com.apple.developer.associated-domains", Some(arr(&["applinks:example.com"])))]),
            ..Default::default()
        },
        Case {
            name: "a granted app group",
            profile_set: vec![("com.apple.security.application-groups", arr(&["group.com.example"]))],
            requested: vec![("com.apple.security.application-groups", arr(&["group.com.example"]))],
            expect: Ok(vec![("com.apple.security.application-groups", Some(arr(&["group.com.example"])))]),
            ..Default::default()
        },
        Case {
            name: "an app group the profile does not list",
            profile_set: vec![("com.apple.security.application-groups", arr(&["group.com.example"]))],
            requested: vec![("com.apple.security.application-groups", arr(&["group.other"]))],
            expect: Err("ValueNotAllowed"),
            ..Default::default()
        },
        Case {
            name: "keychain groups within the team wildcard",
            requested: vec![("keychain-access-groups", arr(&["ABCDE12345.com.example.shared", "com.apple.token"]))],
            expect: Ok(vec![(
                "keychain-access-groups",
                Some(arr(&["ABCDE12345.com.example.shared", "com.apple.token"])),
            )]),
            ..Default::default()
        },
        Case {
            name: "keychain groups of another team",
            requested: vec![("keychain-access-groups", arr(&["ZZZZZ99999.foo"]))],
            expect: Err("ValueNotAllowed"),
            ..Default::default()
        },
        Case {
            name: "keychain groups requested but the profile grants none",
            profile_remove: vec!["keychain-access-groups"],
            requested: vec![("keychain-access-groups", arr(&["ABCDE12345.com.example.app"]))],
            expect: Err("NotInProfile"),
            ..Default::default()
        },
        Case {
            name: "no default keychain group when the profile grants none",
            profile_remove: vec!["keychain-access-groups"],
            expect: Ok(vec![("keychain-access-groups", None)]),
            ..Default::default()
        },
        Case {
            name: "no team identifier when the profile has none",
            profile_remove: vec!["com.apple.developer.team-identifier"],
            expect: Ok(vec![("com.apple.developer.team-identifier", None)]),
            ..Default::default()
        },
        Case {
            name: "a granted boolean",
            profile_set: vec![("com.apple.developer.healthkit", b(true))],
            requested: vec![("com.apple.developer.healthkit", b(true))],
            expect: Ok(vec![("com.apple.developer.healthkit", Some(b(true)))]),
            ..Default::default()
        },
        Case {
            name: "a boolean the profile denies",
            profile_set: vec![("com.apple.developer.healthkit", b(false))],
            requested: vec![("com.apple.developer.healthkit", b(true))],
            expect: Err("ValueNotAllowed"),
            ..Default::default()
        },
        Case {
            name: "a boolean requested false is always allowed",
            profile_set: vec![("com.apple.developer.homekit", b(true))],
            requested: vec![("com.apple.developer.homekit", b(false))],
            expect: Ok(vec![("com.apple.developer.homekit", Some(b(false)))]),
            ..Default::default()
        },
        Case {
            name: "an equal string",
            profile_set: vec![("com.apple.developer.default-data-protection", s("NSFileProtectionComplete"))],
            requested: vec![("com.apple.developer.default-data-protection", s("NSFileProtectionComplete"))],
            expect: Ok(vec![("com.apple.developer.default-data-protection", Some(s("NSFileProtectionComplete")))]),
            ..Default::default()
        },
        Case {
            name: "a different string",
            profile_set: vec![("com.apple.developer.default-data-protection", s("NSFileProtectionComplete"))],
            requested: vec![("com.apple.developer.default-data-protection", s("NSFileProtectionNone"))],
            expect: Err("ValueNotAllowed"),
            ..Default::default()
        },
        Case {
            name: "a string under a wildcard grant",
            profile_set: vec![("com.apple.developer.ubiquity-kvstore-identifier", s("ABCDE12345.*"))],
            requested: vec![(
                "com.apple.developer.ubiquity-kvstore-identifier",
                s("$(TeamIdentifierPrefix)$(CFBundleIdentifier)"),
            )],
            expect: Ok(vec![(
                "com.apple.developer.ubiquity-kvstore-identifier",
                Some(s("ABCDE12345.com.example.app")),
            )]),
            ..Default::default()
        },
        Case {
            name: "a dictionary subset",
            profile_set: vec![("com.example.settings", dict(&[("a", b(true)), ("b", s("x"))]))],
            requested: vec![("com.example.settings", dict(&[("a", b(true))]))],
            expect: Ok(vec![("com.example.settings", Some(dict(&[("a", b(true))])))]),
            ..Default::default()
        },
        Case {
            name: "a dictionary with a key the profile lacks",
            profile_set: vec![("com.example.settings", dict(&[("a", b(true))]))],
            requested: vec![("com.example.settings", dict(&[("a", b(true)), ("c", b(true))]))],
            expect: Err("ValueNotAllowed"),
            ..Default::default()
        },
        Case {
            name: "an equal integer",
            profile_set: vec![("com.example.level", Value::Integer(3.into()))],
            requested: vec![("com.example.level", Value::Integer(3.into()))],
            expect: Ok(vec![("com.example.level", Some(Value::Integer(3.into())))]),
            ..Default::default()
        },
        Case {
            name: "a different integer",
            profile_set: vec![("com.example.level", Value::Integer(3.into()))],
            requested: vec![("com.example.level", Value::Integer(4.into()))],
            expect: Err("ValueNotAllowed"),
            ..Default::default()
        },
        Case {
            name: "least privilege: profile-only keys are not copied",
            profile_set: vec![("com.apple.developer.healthkit", b(true))],
            expect: Ok(vec![("com.apple.developer.healthkit", None)]),
            ..Default::default()
        },
        Case {
            name: "profile-all copies profile-only keys",
            policy: MergePolicy::ProfileAll,
            profile_set: vec![("com.apple.developer.healthkit", b(true))],
            expect: Ok(vec![("com.apple.developer.healthkit", Some(b(true)))]),
            ..Default::default()
        },
        Case {
            name: "profile-all resolves a team wildcard to the bundle",
            policy: MergePolicy::ProfileAll,
            profile_set: vec![("com.apple.developer.ubiquity-kvstore-identifier", s("ABCDE12345.*"))],
            expect: Ok(vec![(
                "com.apple.developer.ubiquity-kvstore-identifier",
                Some(s("ABCDE12345.com.example.app")),
            )]),
            ..Default::default()
        },
        Case {
            name: "profile-all drops a bare wildcard",
            policy: MergePolicy::ProfileAll,
            profile_set: vec![("com.apple.developer.associated-domains", s("*"))],
            expect: Ok(vec![("com.apple.developer.associated-domains", None)]),
            ..Default::default()
        },
        Case {
            name: "an unknown key the profile lacks",
            requested: vec![("com.example.private-thing", b(true))],
            expect: Err("NotInProfile"),
            ..Default::default()
        },
        Case {
            name: "an iCloud key the profile lacks",
            requested: vec![("com.apple.developer.icloud-container-identifiers", arr(&["iCloud.com.example.app"]))],
            expect: Err("NotInProfile"),
            ..Default::default()
        },
        Case {
            name: "get-task-allow must be a boolean",
            requested: vec![("get-task-allow", s("yes"))],
            expect: Err("ValueNotAllowed"),
            ..Default::default()
        },
        Case {
            name: "a string requested against an array grant",
            profile_set: vec![(
                "com.apple.developer.icloud-container-environment",
                arr(&["Development", "Production"]),
            )],
            requested: vec![("com.apple.developer.icloud-container-environment", s("Production"))],
            expect: Ok(vec![("com.apple.developer.icloud-container-environment", Some(s("Production")))]),
            ..Default::default()
        },
        Case {
            name: "an App Store profile without beta-reports-active",
            kind: Kind::Store,
            dist: Distribution::AppStore,
            profile_remove: vec!["beta-reports-active"],
            expect: Ok(vec![("beta-reports-active", None)]),
            ..Default::default()
        },
    ]
}

fn profile_for(c: &Case) -> ProvisioningProfile {
    let mut pb = match c.kind {
        Kind::Dev => ProfileBuilder::development(T, c.pattern),
        Kind::Store => ProfileBuilder::app_store(T, c.pattern),
        Kind::AdHoc => ProfileBuilder::ad_hoc(T, c.pattern),
    };
    for (k, v) in &c.profile_set {
        pb = pb.entitlement(k, v.clone());
    }
    for k in &c.profile_remove {
        pb = pb.without_entitlement(k);
    }
    ProvisioningProfile::parse(&pb.build()).unwrap()
}

fn variant(e: &EntitlementsError) -> &'static str {
    match e {
        EntitlementsError::UnresolvedVariable { .. } => "UnresolvedVariable",
        EntitlementsError::NotInProfile { .. } => "NotInProfile",
        EntitlementsError::ValueNotAllowed { .. } => "ValueNotAllowed",
        EntitlementsError::AppIdMismatch { .. } => "AppIdMismatch",
    }
}

#[test]
fn merge_cases() {
    let all = cases();
    assert!(all.len() >= 40, "PLAN W6 asks for at least 40 cases, have {}", all.len());
    for c in &all {
        let profile = profile_for(c);
        let mut requested = Dictionary::new();
        for (k, v) in &c.requested {
            requested.insert((*k).into(), v.clone());
        }
        let resolver =
            EntitlementsResolver { profile: &profile, bundle_id: c.bundle, distribution: c.dist, policy: c.policy };
        match (&c.expect, resolver.resolve(&requested)) {
            (Expect::Ok(checks), Ok(r)) => {
                for (k, v) in checks {
                    assert_eq!(r.dict.get(k), v.as_ref(), "{}: key {k}", c.name);
                }
            }
            (Expect::Err(name), Err(e)) => assert_eq!(variant(&e), *name, "{}: got {e}", c.name),
            (Expect::Ok(_), Err(e)) => panic!("{}: unexpected error {e}", c.name),
            (Expect::Err(name), Ok(r)) => panic!("{}: expected {name}, got {:?}", c.name, r.dict),
        }
    }
}

#[test]
fn notes_and_error_codes() {
    let profile = ProvisioningProfile::parse(
        &ProfileBuilder::development(T, "com.example.app").entitlement("aps-environment", s("production")).build(),
    )
    .unwrap();
    let mut req = Dictionary::new();
    req.insert("aps-environment".into(), s("development"));
    let r = EntitlementsResolver {
        profile: &profile,
        bundle_id: "com.example.app",
        distribution: Distribution::Development,
        policy: MergePolicy::Least,
    }
    .resolve(&req)
    .unwrap();
    assert_eq!(r.notes.len(), 1);
    assert_eq!(r.notes[0].key, "aps-environment");

    let e: russet_core::Error = EntitlementsError::NotInProfile {
        key: "aps-environment".into(),
        capability_hint: Some("PUSH_NOTIFICATIONS".into()),
    }
    .into();
    assert_eq!(e.code(), codes::sign::ENTITLEMENT_NOT_IN_PROFILE);
    assert!(e.hint().unwrap().contains("PUSH_NOTIFICATIONS"));
    let e: russet_core::Error = EntitlementsError::AppIdMismatch { profile: "x".into(), bundle: "y".into() }.into();
    assert_eq!(e.code(), codes::sign::PROFILE_MISMATCH);
    let e: russet_core::Error = EntitlementsError::UnresolvedVariable { key: "k".into(), var: "V".into() }.into();
    assert_eq!(e.code(), codes::sign::ENTITLEMENT_UNRESOLVED_VARIABLE);
    let e: russet_core::Error =
        EntitlementsError::ValueNotAllowed { key: "k".into(), requested: "1".into(), allowed: "2".into() }.into();
    assert_eq!(e.code(), codes::sign::ENTITLEMENT_VALUE_NOT_ALLOWED);
    assert!(
        russet_sign::capability_for_entitlement("aps-environment")
            .is_some_and(|c| c.capability == "PUSH_NOTIFICATIONS")
    );
    assert!(russet_sign::capability_for_entitlement("keychain-access-groups").is_none());
}
