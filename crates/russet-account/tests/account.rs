// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Accounts, the xtool import, provisioning plans and signing with stored identities, against a synthetic xtool
//! store and a wiremock App Store Connect fake (every request a GET scoped to itself).

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use common::*;
use russet_account::*;
use russet_core::error::codes;
use russet_sign::{CodeSigningIdentity as _, Distribution, VerifyOptions};

fn service(e: &Env, asc: &str) -> AccountService {
    AccountService::with_xtool_dir(&e.dirs, e.secrets.clone(), endpoints(asc), Some(e.xtool.clone())).unwrap()
}

#[tokio::test]
async fn imports_xtool_once_and_seals_everything() {
    let e = env();
    write_xtool(&e.xtool, true);
    let svc = service(&e, "http://127.0.0.1:9/");

    let dry = svc.accounts().write().await.import_xtool(true).await.unwrap();
    assert!(dry.dry_run);
    assert_eq!(dry.accounts.len(), 1);
    assert_eq!(dry.identities.len(), 1);
    assert!(e.secrets.items.lock().unwrap().is_empty(), "a dry run seals nothing");
    assert!(!e.dirs.config.join("accounts.toml").exists());

    let imp = svc.accounts().write().await.import_xtool(false).await.unwrap();
    let acc = &imp.accounts[0];
    assert_eq!(acc.id, AccountId::asc(KEY_ID));
    assert_eq!(acc.kind, AccountKind::AscTeamKey);
    assert_eq!(acc.team_id.as_deref(), Some(TEAM), "the team comes from the identity certificate's OU");
    assert_eq!(acc.imported_from.as_deref(), Some("xtool"));
    assert!(acc.is_default);
    assert_eq!(acc.xtool_prefix().as_deref(), Some(PREFIX));
    assert!(imp.warnings.iter().any(|w| w.contains("chmod 600")), "{:?}", imp.warnings);
    assert!(imp.warnings.iter().any(|w| w.contains("xtool auth logout")));

    let sealed: Vec<String> = e.secrets.items.lock().unwrap().keys().cloned().collect();
    let id = &imp.identities[0];
    assert!(sealed.contains(&format!("asc-key/{KEY_ID}")));
    assert!(sealed.contains(&format!("signing-key/{}", id.id)));
    assert_eq!(id.team_id, TEAM);
    assert_eq!(id.kind, CertKind::Development);
    assert_eq!(id.imported_from.as_deref(), Some("xtool"));

    // The identity loads from the store (PKCS#1 RSA key, as xtool keeps it) and matches its certificate.
    let loaded = svc.identities().load(&id.id).await.unwrap();
    assert_eq!(loaded.certificate_der(), &rsa_identity().0.cert_der[..]);
    assert_eq!(loaded.team_id(), TEAM);
    assert_eq!(svc.identities().list(Some(TEAM)).unwrap().len(), 1);
    assert!(svc.identities().list(Some("ZZZZZ99999")).unwrap().is_empty());

    // accounts.toml: kebab-case, private, re-readable; a second import replaces, never duplicates.
    let text = std::fs::read_to_string(e.dirs.config.join("accounts.toml")).unwrap();
    for key in
        ["schema = 1", "key-id", "issuer-id", "team-id", "imported-from = \"xtool\"", "secret = \"secret:asc-key/"]
    {
        assert!(text.contains(key), "{key} in accounts.toml");
    }
    assert!(!text.contains("PRIVATE KEY"), "no key material in accounts.toml");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(e.dirs.config.join("accounts.toml")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    svc.accounts().write().await.import_xtool(false).await.unwrap();
    let reloaded = AccountStore::load(&e.dirs, e.secrets.clone()).unwrap();
    assert_eq!(reloaded.list(), svc.accounts().read().await.list());
    assert_eq!(reloaded.list().len(), 1);
}

#[tokio::test]
async fn import_reports_what_it_cannot_take() {
    let e = env();
    let svc = service(&e, "http://127.0.0.1:9/");
    let err = svc.accounts().write().await.import_xtool(false).await.unwrap_err();
    assert_eq!(err.code(), codes::core::NOT_FOUND, "no xtool login");
    assert!(err.hint().unwrap().contains("xtool auth"));

    // An Apple ID session is not imported in Wave 1.
    std::fs::write(
        e.xtool.join("XTLAuthToken"),
        br#"{"xcode":{"_0":{"appleID":"x","adsid":"y","token":"z","expiry":0,"teamID":"T"}}}"#,
    )
    .unwrap();
    let err = svc.accounts().write().await.import_xtool(false).await.unwrap_err();
    assert_eq!(err.code(), codes::core::NOT_FOUND);

    // A key without an identity yet imports the key and says so; open modes get the chmod advice.
    write_xtool(&e.xtool, false);
    set_mode(&e.xtool.join("XTLAuthToken"), 0o644);
    let imp = svc.accounts().write().await.import_xtool(false).await.unwrap();
    assert_eq!(imp.accounts.len(), 1);
    assert!(imp.identities.is_empty());
    assert!(imp.accounts[0].team_id.is_none());
    assert!(imp.warnings.iter().any(|w| w.contains("no development identity")));
    #[cfg(unix)]
    assert!(
        imp.warnings.iter().any(|w| w.contains("readable by other users") && w.contains("chmod 600")),
        "{:?}",
        imp.warnings
    );

    // A broken key is refused before anything is sealed.
    std::fs::write(e.xtool.join("XTLAuthToken"), br#"{"appStoreConnect":{"_0":{"id":"2X9R4HXF34","issuerID":"69a6de7d-1111-2222-3333-444455556666","pem":"not a key"}}}"#).unwrap();
    let e2 = env();
    std::fs::copy(e.xtool.join("XTLAuthToken"), e2.xtool.join("XTLAuthToken")).unwrap();
    let svc2 = service(&e2, "http://127.0.0.1:9/");
    assert_eq!(
        svc2.accounts().write().await.import_xtool(false).await.unwrap_err().code(),
        codes::account::KEY_INVALID
    );
    assert!(e2.secrets.items.lock().unwrap().is_empty());
}

#[tokio::test]
async fn adds_a_key_and_discovers_the_team() {
    let e = env();
    let (server, fake) = fake_asc(xtool_team("com.example.hello")).await;
    let svc = service(&e, &format!("{}/", server.uri()));
    let p8 = e.tmp.path().join("AuthKey.p8");
    std::fs::write(&p8, p8_pem()).unwrap();
    let acc = svc
        .accounts()
        .write()
        .await
        .add_asc_key(AddAscKey {
            key_id: KEY_ID.into(),
            issuer_id: Some(ISSUER.into()),
            p8_path: p8.clone(),
            label: None,
        })
        .await
        .unwrap();
    assert_eq!(acc.team_id.as_deref(), Some(TEAM), "from a certificate's subject OU");
    assert!(acc.imported_from.is_none());
    assert_eq!(fake.non_get_requests(), 0);

    // A wrong issuer is rejected by App Store Connect: nothing is stored.
    let e2 = env();
    let svc2 = service(&e2, &format!("{}/", server.uri()));
    let err = svc2
        .accounts()
        .write()
        .await
        .add_asc_key(AddAscKey {
            key_id: KEY_ID.into(),
            issuer_id: Some("00000000-0000-0000-0000-000000000000".into()),
            p8_path: p8,
            label: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), codes::account::ASC_UNAUTHORIZED);
    assert!(svc2.accounts().read().await.list().is_empty());
    let bad = e.tmp.path().join("bad.p8");
    std::fs::write(&bad, "nope").unwrap();
    let err = svc2
        .accounts()
        .write()
        .await
        .add_asc_key(AddAscKey { key_id: KEY_ID.into(), issuer_id: Some(ISSUER.into()), p8_path: bad, label: None })
        .await
        .unwrap_err();
    assert_eq!(err.code(), codes::account::KEY_INVALID);
}

#[tokio::test]
async fn verify_probes_with_scoped_reads() {
    let e = env();
    write_xtool(&e.xtool, true);
    let mut state = xtool_team("com.example.hello");
    state.forbidden = vec!["/v1/builds".into()];
    let (server, fake) = fake_asc(state).await;
    let svc = service(&e, &format!("{}/", server.uri()));
    svc.accounts().write().await.import_xtool(false).await.unwrap();
    let v = svc.verify(None).await.unwrap();
    assert!(v.ok);
    assert_eq!(v.team_id.as_deref(), Some(TEAM));
    assert_eq!(
        (v.certificates.clone(), v.profiles.clone(), v.builds.clone()),
        (ProbeResult::Ok, ProbeResult::Ok, ProbeResult::Forbidden)
    );
    assert_eq!(fake.non_get_requests(), 0);
    assert_eq!(svc.verify(Some(&AccountId("asc:NOPE".into()))).await.unwrap_err().code(), codes::account::NO_ACCOUNT);
}

/// The Wave-1 acceptance shape (PLAN §4.6): the owner's `russet.local.toml` maps the app onto the bundle ID xtool
/// registered (`xtool-prefix`), and the plan reuses xtool's profile with zero mutations.
#[tokio::test]
async fn plans_zero_mutations_for_the_existing_xtool_profile() {
    let e = env();
    write_xtool(&e.xtool, true);
    let bundle = format!("XTL-{PREFIX}.com.example.hello");
    let (server, fake) = fake_asc(xtool_team(&bundle)).await;
    let svc = service(&e, &format!("{}/", server.uri()));
    svc.accounts().write().await.import_xtool(false).await.unwrap();
    let proj = project(
        e.tmp.path(),
        "dev.russet.test.probe",
        Some("[app]\nbundle-id = \"com.example.hello\"\n[signing]\nbundle-id-policy = \"xtool-prefix\"\n"),
    );
    let plan = svc.plan_project(&proj, Distribution::Development, vec![UDID.into()], None).await.unwrap();
    assert!(plan.mutations.is_empty(), "{:?}", plan.mutations);
    assert_eq!(plan.profiles.len(), 1);
    assert_eq!(plan.profiles[0].bundle_id, bundle);
    assert_eq!(plan.profiles[0].name, format!("XTL {bundle}"));
    assert!(plan.identity.is_some());
    assert!(plan.plan_id.0.starts_with("pln_"));
    assert_eq!(fake.non_get_requests(), 0, "planning is GET-only");

    // Applying a zero-mutation plan needs no approval and caches the profile for signing.
    let before = fake.count();
    let out = svc.apply_plan(&plan.plan_id.0, &ApprovalRef::default()).await.unwrap();
    assert!(fake.count() > before);
    assert_eq!(out.bundles.len(), 1);
    assert!(out.bundles[0].profile_path.is_file());
    assert_eq!(svc.profiles().index().unwrap().len(), 1);
    assert_eq!(fake.non_get_requests(), 0);

    // The cached profile signs the app with the imported identity (the pipeline's sign step).
    let app = e.tmp.path().join("Hello.app");
    std::fs::create_dir_all(&app).unwrap();
    let mut info = plist::Dictionary::new();
    for (k, v) in
        [("CFBundleIdentifier", bundle.as_str()), ("CFBundleExecutable", "Hello"), ("CFBundlePackageType", "APPL")]
    {
        info.insert(k.into(), plist::Value::String(v.into()));
    }
    plist::Value::Dictionary(info).to_file_xml(app.join("Info.plist")).unwrap();
    std::fs::write(app.join("Hello"), russet_macho::fixture::MachOBuilder::executable().build()).unwrap();
    let mut req = SignAppRequest::new(app.clone());
    req.output = Some(e.tmp.path().join("signed/Hello.app"));
    req.get_task_allow = Some(true);
    req.device = Some(UDID.into());
    req.lint = None;
    let report = svc.sign_app(req).await.unwrap();
    assert_eq!(report.bundles.len(), 1);
    let v = russet_sign::verify(&report.output, &VerifyOptions { device: Some(UDID.into()), ..Default::default() })
        .unwrap();
    assert!(v.ok, "{:#?}", v.findings);

    // Without a cached profile for the bundle, signing explains what to do.
    let other = e.tmp.path().join("Other.app");
    std::fs::create_dir_all(&other).unwrap();
    let mut info = plist::Dictionary::new();
    for (k, v) in
        [("CFBundleIdentifier", "com.other.app"), ("CFBundleExecutable", "Hello"), ("CFBundlePackageType", "APPL")]
    {
        info.insert(k.into(), plist::Value::String(v.into()));
    }
    plist::Value::Dictionary(info).to_file_xml(other.join("Info.plist")).unwrap();
    std::fs::write(other.join("Hello"), russet_macho::fixture::MachOBuilder::executable().build()).unwrap();
    let err = svc.sign_app(SignAppRequest::new(other)).await.unwrap_err();
    assert_eq!(err.code(), codes::sign::PROFILE_MISMATCH);
    assert!(err.hint().unwrap().contains("russet provision"));
}

#[tokio::test]
async fn plans_list_every_needed_change_and_apply_refuses_them() {
    let e = env();
    write_xtool(&e.xtool, true);
    let bundle = "com.example.hello";
    let mut state = xtool_team(bundle);
    state.devices.clear();
    let (server, fake) = fake_asc(state).await;
    let svc = service(&e, &format!("{}/", server.uri()));
    svc.accounts().write().await.import_xtool(false).await.unwrap();

    // Push is requested but not enabled; the device is not registered; a second bundle does not exist.
    let proj = project(e.tmp.path(), bundle, None);
    std::fs::write(
        proj.join("russet.toml"),
        format!(
            "schema = 1\n[app]\nbundle-id = \"{bundle}\"\nentitlements = \"App.entitlements\"\n[[app.extensions]]\nproduct = \"Widget\"\nbundle-id-suffix = \"widget\"\n"
        ),
    )
    .unwrap();
    let mut ents = plist::Dictionary::new();
    ents.insert("aps-environment".into(), plist::Value::String("development".into()));
    plist::Value::Dictionary(ents).to_file_xml(proj.join("App.entitlements")).unwrap();
    let plan = svc.plan_project(&proj, Distribution::Development, vec![UDID.into()], None).await.unwrap();
    let kinds: Vec<&str> = plan.mutations.iter().map(Mutation::kind).collect();
    assert!(kinds.contains(&"register-device"), "{kinds:?}");
    assert!(kinds.contains(&"enable-capability"));
    assert!(kinds.contains(&"register-bundle-id"));
    assert_eq!(kinds.iter().filter(|k| **k == "create-profile").count(), 2);
    assert!(
        plan.mutations
            .iter()
            .any(|m| matches!(m, Mutation::RegisterDevice { udid_redacted } if !udid_redacted.contains("1B2C3D"))),
        "UDIDs are redacted"
    );
    assert!(plan.mutations.iter().any(
        |m| matches!(m, Mutation::RegisterBundleId { identifier, .. } if identifier == "com.example.hello.widget")
    ));
    assert_eq!(fake.non_get_requests(), 0);

    let err = svc.apply_plan(&plan.plan_id.0, &ApprovalRef::default()).await.unwrap_err();
    assert_eq!(err.code(), codes::perm::APPROVAL_REQUIRED);
    assert_eq!(err.code().exit_code(), 11);
    let approved =
        ApprovalRef { approval_id: Some(serde_json::from_str("\"apr_01J00000000000000000000000\"").unwrap()) };
    assert_eq!(svc.apply_plan(&plan.plan_id.0, &approved).await.unwrap_err().code(), codes::core::NOT_IMPLEMENTED);
    assert_eq!(fake.non_get_requests(), 0, "nothing was changed on the account");
    assert_eq!(
        svc.apply_plan("pln_unknown", &ApprovalRef::default()).await.unwrap_err().code(),
        codes::core::NOT_FOUND
    );
}

#[tokio::test]
async fn a_changed_account_makes_the_plan_stale_and_missing_identities_need_a_certificate() {
    let e = env();
    write_xtool(&e.xtool, true);
    let bundle = "com.example.hello";
    let (server, fake) = fake_asc(xtool_team(bundle)).await;
    let svc = service(&e, &format!("{}/", server.uri()));
    svc.accounts().write().await.import_xtool(false).await.unwrap();
    let proj = project(e.tmp.path(), bundle, None);
    let plan = svc.plan_project(&proj, Distribution::Development, vec![], None).await.unwrap();
    assert!(plan.mutations.is_empty());
    assert!(plan.warnings.iter().any(|w| w.message.contains("no device")));
    fake.state.lock().unwrap().profiles.clear();
    let err = svc.apply_plan(&plan.plan_id.0, &ApprovalRef::default()).await.unwrap_err();
    assert_eq!(err.code(), codes::provision::PLAN_STALE);

    // A team whose portal lists none of our certificates needs a new one (a mutation W12 performs).
    fake.state.lock().unwrap().certificates.clear();
    let plan = svc.plan_project(&proj, Distribution::Development, vec![], None).await.unwrap();
    assert!(plan.mutations.iter().any(|m| matches!(m, Mutation::CreateCertificate { kind: CertKind::Development })));
    assert!(plan.identity.is_none());
}

#[test]
fn reads_project_signing_inputs() {
    let e = env();
    let none = e.tmp.path().join("empty");
    std::fs::create_dir_all(&none).unwrap();
    assert_eq!(ProjectSigning::read(&none).unwrap_err().code(), codes::config::PROJECT_NOT_FOUND);
    let broken = project(e.tmp.path(), "x", Some("[signing\n"));
    assert_eq!(ProjectSigning::read(&broken).unwrap_err().code(), codes::config::INVALID);

    let p = project(
        e.tmp.path(),
        "com.example.hello",
        Some(
            "[signing]\nbundle-id-policy = { custom = { \"com.example.hello\" = \"com.mine.hello\" } }\naccount = \"asc:K\"\n",
        ),
    );
    let s = ProjectSigning::read(&p).unwrap();
    assert_eq!(s.account.as_deref(), Some("asc:K"));
    assert_eq!(s.effective_bundle_id("com.example.hello", None, None).unwrap(), "com.mine.hello");

    let acc = Account {
        id: AccountId::asc(KEY_ID),
        kind: AccountKind::AscTeamKey,
        label: String::new(),
        key_id: Some(KEY_ID.into()),
        issuer_id: Some(ISSUER.into()),
        team_id: Some(TEAM.into()),
        secret: format!("secret:asc-key/{KEY_ID}"),
        imported_from: Some("xtool".into()),
        is_default: true,
    };
    let mut s2 = s.clone();
    s2.policy = BundleIdPolicySpec::XtoolPrefix;
    assert_eq!(
        s2.effective_bundle_id("com.example.hello", Some(&acc), None).unwrap(),
        format!("XTL-{PREFIX}.com.example.hello")
    );
    s2.policy = BundleIdPolicySpec::TeamSuffix;
    assert_eq!(s2.effective_bundle_id("a.b", None, Some(TEAM)).unwrap(), format!("a.b.{TEAM}"));
    assert_eq!(s2.effective_bundle_id("a.b", None, None).unwrap_err().code(), codes::account::NO_ACCOUNT);
    s2.policy = BundleIdPolicySpec::Exact;
    assert_eq!(s2.effective_bundle_id("a.b", None, None).unwrap(), "a.b");
}

#[tokio::test]
async fn doctor_checks_read_files_only() {
    use russet_doctor::{Check as _, CheckCx, Status};
    let e = env();
    let cx = CheckCx::default();
    let configured = AccountConfigured { dirs: Some(e.dirs.clone()) };
    assert_eq!(configured.run(&cx).await.status, Status::Warn);
    let identity = IdentityUsable { dirs: Some(e.dirs.clone()) };
    assert_eq!(identity.run(&cx).await.status, Status::Warn);
    let files = XtoolFilesPrivate { dir: Some(e.xtool.clone()) };
    write_xtool(&e.xtool, true);
    assert_eq!(files.run(&cx).await.status, Status::Pass);
    #[cfg(unix)]
    {
        set_mode(&e.xtool.join("XTLAuthToken"), 0o644);
        let r = files.run(&cx).await;
        assert_eq!(r.status, Status::Warn);
        assert!(r.hint.unwrap().starts_with("chmod 600"));
        set_mode(&e.xtool.join("XTLAuthToken"), 0o600);
    }
    let svc = service(&e, "http://127.0.0.1:9/");
    svc.accounts().write().await.import_xtool(false).await.unwrap();
    assert_eq!(configured.run(&cx).await.status, Status::Pass);
    assert_eq!(identity.run(&cx).await.status, Status::Pass);
    assert_eq!(checks().len(), 3);
    let _: Arc<dyn russet_doctor::Check> = Arc::new(XtoolFilesPrivate::default());
}

/// The redaction canary (DoD: no secrets in logs): after an import, no Debug output, file or wire-bound value
/// carries a byte of the App Store Connect key or the signing key.
#[tokio::test]
async fn secrets_never_leak_into_debug_output_files_or_errors() {
    let e = env();
    write_xtool(&e.xtool, true);
    let svc = service(&e, "http://127.0.0.1:9/");
    let imp = svc.accounts().write().await.import_xtool(false).await.unwrap();
    let canaries: Vec<String> = [p8_pem(), rsa_identity().1.clone()]
        .iter()
        .flat_map(|pem| {
            pem.lines().filter(|l| !l.starts_with("-----") && l.len() > 20).map(str::to_owned).collect::<Vec<_>>()
        })
        .collect();
    assert!(!canaries.is_empty());
    let id = &imp.identities[0];
    let loaded = svc.identities().load(&id.id).await.unwrap();
    let key = svc.accounts().read().await.asc_key(&imp.accounts[0]).await.unwrap();
    let mut seen = vec![
        format!("{imp:?}"),
        format!("{:?}", svc.accounts().read().await),
        format!("{svc:?}"),
        format!("{loaded:?}"),
        format!("{key:?}"),
        format!("{:?}", svc.identities()),
        std::fs::read_to_string(e.dirs.config.join("accounts.toml")).unwrap(),
    ];
    for f in std::fs::read_dir(e.dirs.data.join("identities").join(TEAM)).unwrap() {
        let p = f.unwrap().path();
        if p.extension().is_some_and(|x| x == "json") {
            seen.push(std::fs::read_to_string(p).unwrap());
        }
    }
    // Errors on the paths that touch the key carry no key material either.
    e.secrets.items.lock().unwrap().remove(&format!("asc-key/{KEY_ID}"));
    let err = svc.accounts().read().await.asc_key(&imp.accounts[0]).await.unwrap_err();
    seen.push(format!("{err:?} {}", err.message()));
    seen.push(serde_json::to_string(&err.to_data()).unwrap());
    for text in &seen {
        assert!(!text.contains("PRIVATE KEY"), "a PEM header leaked");
        for c in &canaries {
            assert!(!text.contains(c.as_str()), "key material leaked");
        }
    }
}
