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
    assert!(sealed.contains(&format!("signing-key/{}", id.sha1)), "named by the certificate's SHA-1, not the team");
    assert!(!sealed.iter().any(|k| k.contains(TEAM)), "secret names never name the team: {sealed:?}");
    assert_eq!(id.team_id, TEAM);
    assert_eq!(id.kind, CertKind::Development);
    assert_eq!(id.imported_from.as_deref(), Some("xtool"));

    // The identity loads from the store (PKCS#1 RSA key, as xtool keeps it) and matches its certificate. `load`
    // is ARCHITECTURE §5.3's synchronous form, called here from inside a (current-thread) runtime as the run
    // pipeline does; `load_async` is the same read for async callers.
    let loaded = svc.identities().load(&id.id).unwrap();
    assert_eq!(svc.identities().load_async(&id.id).await.unwrap().certificate_der(), loaded.certificate_der());
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

    // The pipeline's form: a product built with the canonical ID is signed as the provisioned (effective) one.
    let canonical = e.tmp.path().join("Canonical.app");
    std::fs::create_dir_all(&canonical).unwrap();
    let mut info = plist::Dictionary::new();
    for (k, v) in
        [("CFBundleIdentifier", "com.example.hello"), ("CFBundleExecutable", "Hello"), ("CFBundlePackageType", "APPL")]
    {
        info.insert(k.into(), plist::Value::String(v.into()));
    }
    plist::Value::Dictionary(info).to_file_xml(canonical.join("Info.plist")).unwrap();
    std::fs::write(canonical.join("Hello"), russet_macho::fixture::MachOBuilder::executable().build()).unwrap();
    let mut req = SignAppRequest::from_outcome(canonical.clone(), Distribution::Development, &out);
    assert_eq!(req.bundle_ids.get(""), Some(&bundle));
    req.output = Some(e.tmp.path().join("signed/Canonical.app"));
    req.get_task_allow = Some(true);
    req.lint = None;
    let report = svc.sign_app(req).await.unwrap();
    assert_eq!(report.bundles[0].bundle_id, bundle);
    let id_of = |p: &std::path::Path| {
        plist::Value::from_file(p.join("Info.plist")).unwrap().into_dictionary().unwrap()["CFBundleIdentifier"]
            .as_string()
            .unwrap()
            .to_owned()
    };
    assert_eq!(id_of(&report.output), bundle);
    assert_eq!(id_of(&canonical), "com.example.hello", "the unsigned product keeps its ID");
    assert!(russet_sign::verify(&report.output, &VerifyOptions::default()).unwrap().ok);
    let mut bad = SignAppRequest::from_outcome(canonical, Distribution::Development, &out);
    bad.bundle_ids.insert("PlugIns/Nope.appex".into(), "x.y".into());
    assert_eq!(svc.sign_app(bad).await.unwrap_err().code(), codes::sign::BUNDLE_INVALID);

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
    let loaded = svc.identities().load_async(&id.id).await.unwrap();
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

fn write_p8(e: &Env) -> std::path::PathBuf {
    let p8 = e.tmp.path().join("AuthKey.p8");
    std::fs::write(&p8, p8_pem()).unwrap();
    p8
}

/// Individual keys cannot use the provisioning endpoints (SIGN §5.2): adding one never probes them, `verify`
/// calls it usable when it reads builds, and planning says why it cannot provision.
#[tokio::test]
async fn individual_keys_are_added_without_provisioning() {
    let e = env();
    let mut state = xtool_team("com.example.hello");
    state.individual = true;
    let (server, fake) = fake_asc(state).await;
    let svc = service(&e, &format!("{}/", server.uri()));
    let acc = svc
        .accounts()
        .write()
        .await
        .add_asc_key(AddAscKey { key_id: KEY_ID.into(), issuer_id: None, p8_path: write_p8(&e), label: None })
        .await
        .unwrap();
    assert_eq!(acc.kind, AccountKind::AscIndividualKey);
    assert!(acc.team_id.is_none());
    assert!(!fake.paths().iter().any(|p| p.starts_with("/v1/certificates")), "{:?}", fake.paths());
    assert!(fake.paths().iter().any(|p| p == "/v1/apps"));

    let v = svc.verify(None).await.unwrap();
    assert!(v.ok, "an individual key that reads builds is usable");
    assert_eq!(
        (v.certificates, v.profiles, v.builds),
        (ProbeResult::Forbidden, ProbeResult::Forbidden, ProbeResult::Ok)
    );

    let req = ProvisionRequest {
        bundles: vec![BundleSpec::with_entitlements_file("", "com.example.hello", None).unwrap()],
        distribution: Distribution::Development,
        devices: vec![],
        identity: None,
    };
    let p = svc.provisioner(None, Some(TEAM)).await.unwrap();
    assert_eq!(p.plan(&req).await.unwrap_err().code(), codes::account::INDIVIDUAL_KEY_NO_PROVISIONING);
    assert_eq!(fake.non_get_requests(), 0);
}

/// A team key whose role cannot read certificates is still kept (it can serve TestFlight later), without a team.
#[tokio::test]
async fn a_team_key_without_certificate_access_is_kept_without_a_team() {
    let e = env();
    let mut state = xtool_team("com.example.hello");
    state.forbidden = vec!["/v1/certificates".into(), "/v1/bundleIds".into()];
    let (server, _fake) = fake_asc(state).await;
    let svc = service(&e, &format!("{}/", server.uri()));
    let acc = svc
        .accounts()
        .write()
        .await
        .add_asc_key(AddAscKey {
            key_id: KEY_ID.into(),
            issuer_id: Some(ISSUER.into()),
            p8_path: write_p8(&e),
            label: None,
        })
        .await
        .unwrap();
    assert_eq!(acc.kind, AccountKind::AscTeamKey);
    assert!(acc.team_id.is_none());
    let v = svc.verify(None).await.unwrap();
    assert!(!v.ok, "a team key that cannot read certificates cannot provision");
    assert_eq!(v.certificates, ProbeResult::Forbidden);
}

/// `account.verify` is read-class: it reports the team it discovers but writes nothing.
#[tokio::test]
async fn verify_writes_nothing() {
    let e = env();
    write_xtool(&e.xtool, false); // a key without an identity: no team yet
    let (server, _fake) = fake_asc(xtool_team("com.example.hello")).await;
    let svc = service(&e, &format!("{}/", server.uri()));
    svc.accounts().write().await.import_xtool(false).await.unwrap();
    let file = e.dirs.config.join("accounts.toml");
    let before = std::fs::read(&file).unwrap();
    let v = svc.verify(None).await.unwrap();
    assert!(v.ok);
    assert_eq!(v.team_id.as_deref(), Some(TEAM), "discovered from a certificate");
    assert_eq!(std::fs::read(&file).unwrap(), before, "accounts.toml is unchanged");
    assert!(svc.accounts().read().await.list()[0].team_id.is_none());
}

struct ManualClock(std::sync::Mutex<std::time::Instant>);

impl ManualClock {
    fn advance(&self, d: std::time::Duration) {
        *self.0.lock().unwrap() += d;
    }
}

impl russet_core::time::Clock for ManualClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::SystemTime::now()
    }
    fn instant(&self) -> std::time::Instant {
        *self.0.lock().unwrap()
    }
}

/// A plan expires after 10 minutes: applying it then is `provision.planStale`, distinct from an unknown id
/// (`core.notFound`); after an hour the daemon forgets it.
#[tokio::test]
async fn expired_plans_are_stale_and_unknown_plans_are_not_found() {
    let e = env();
    write_xtool(&e.xtool, true);
    let (server, _fake) = fake_asc(xtool_team("com.example.hello")).await;
    let clock = Arc::new(ManualClock(std::sync::Mutex::new(std::time::Instant::now())));
    let svc = service(&e, &format!("{}/", server.uri())).with_clock(clock.clone());
    svc.accounts().write().await.import_xtool(false).await.unwrap();
    let proj = project(e.tmp.path(), "com.example.hello", None);
    let plan = svc.plan_project(&proj, Distribution::Development, vec![], None).await.unwrap();
    assert!(svc.plan(&plan.plan_id.0).is_ok());
    clock.advance(PLAN_TTL + std::time::Duration::from_secs(1));
    let err = svc.apply_plan(&plan.plan_id.0, &ApprovalRef::default()).await.unwrap_err();
    assert_eq!(err.code(), codes::provision::PLAN_STALE);
    assert_eq!(err.to_data().details.unwrap()["reason"], "expired");
    assert!(err.hint().unwrap().contains("Plan again"));
    assert_eq!(svc.plan("pln_unknown").unwrap_err().code(), codes::core::NOT_FOUND);
    clock.advance(PLAN_MEMORY);
    let _ = svc.plan_project(&proj, Distribution::Development, vec![], None).await.unwrap(); // purges old plans
    assert_eq!(svc.plan(&plan.plan_id.0).unwrap_err().code(), codes::core::NOT_FOUND);
}

/// One rate limiter per key spans every call (portal reads, verify, add-key), so the hourly quota is shared.
#[tokio::test]
async fn one_rate_limiter_per_key_spans_calls() {
    let e = env();
    write_xtool(&e.xtool, true);
    let (server, fake) = fake_asc(xtool_team("com.example.hello")).await;
    let svc = service(&e, &format!("{}/", server.uri()));
    svc.accounts().write().await.import_xtool(false).await.unwrap();
    let limiter = svc.accounts().read().await.limiter(KEY_ID);
    svc.verify(None).await.unwrap();
    let portal = svc.accounts().read().await.portal(&AccountId::asc(KEY_ID), None).await.unwrap();
    portal.list_devices().await.unwrap();
    let used = f64::from(russet_asc::BURST) - limiter.available();
    assert!(
        used >= fake.count() as f64 - 0.5,
        "every request took a token of the one bucket: {used} vs {}",
        fake.count()
    );
    assert!(Arc::ptr_eq(&limiter, &svc.accounts().read().await.limiter(KEY_ID)));
}

fn p12_of(id: &russet_sign::testpki::TestIdentity, password: &str) -> Vec<u8> {
    let key = p12_keystore::PrivateKey::from_der(&id.key_pkcs8).unwrap();
    let cert = p12_keystore::Certificate::from_der(&id.cert_der).unwrap();
    let mut store = p12_keystore::KeyStore::new();
    store.add_entry(
        "identity",
        p12_keystore::KeyStoreEntry::PrivateKeyChain(p12_keystore::PrivateKeyChain::new(b"k1".as_slice(), key, [cert])),
    );
    store.writer(password).write().unwrap()
}

/// `.p12` import through the identity store, in ARCHITECTURE §5.3's synchronous form and the async one; the
/// identity is then found by its wire id (the certificate serial), its SHA-1 or its `<team>/<serial>` id.
#[tokio::test]
async fn imports_p12_identities_and_resolves_them() {
    let e = env();
    let svc = service(&e, "http://127.0.0.1:9/");
    let (rsa, _) = rsa_identity();
    let pw = russet_secrets::SecretBytes::new(b"p12 password".to_vec());
    let a = svc.identities().import_p12(&p12_of(rsa, "p12 password"), &pw).unwrap();
    assert_eq!((a.team_id.as_str(), a.kind), (TEAM, CertKind::Development));
    let other = russet_sign::testpki::development_identity(TEAM);
    let b = svc.identities().import_p12_async(&p12_of(&other, "p12 password"), &pw).await.unwrap();
    assert_ne!(a.id, b.id);
    let wrong = russet_secrets::SecretBytes::new(b"nope".to_vec());
    assert_eq!(
        svc.identities().import_p12(&p12_of(rsa, "p12 password"), &wrong).unwrap_err().code(),
        codes::sign::NO_IDENTITY
    );

    for name in [a.id.wire(), a.id.wire().to_lowercase(), format!("00{}", a.id.wire()), a.sha1.clone(), a.id.0.clone()]
    {
        assert_eq!(svc.identities().resolve(&name).unwrap().id, a.id, "{name}");
    }
    assert_eq!(a.id.wire(), a.serial, "the wire id is the certificate serial");
    assert!(!a.id.wire().contains(TEAM) && !a.id.masked().contains(TEAM) && !a.id.to_string().contains(TEAM));
    assert_eq!(svc.identities().resolve("FFFFFFFF").unwrap_err().code(), codes::sign::NO_IDENTITY);
    let loaded = svc.identities().load(&a.id).unwrap();
    assert_eq!(russet_sign::CodeSigningIdentity::certificate_der(&loaded), &rsa.cert_der[..]);
    assert_eq!(svc.identities().list(Some("*******345")).unwrap().len(), 2, "the masked team filters too");
}

/// Nothing the account service reports names the team or the issuer: import warnings, plan warnings, and the
/// errors of identity, planning and signing paths (AGENTS.md §8; the daemon masks DTO fields itself).
#[tokio::test]
async fn reports_and_errors_never_name_the_team_or_the_issuer() {
    let e = env();
    write_xtool(&e.xtool, true);
    let bundle = "com.example.hello";
    let (server, fake) = fake_asc(xtool_team(bundle)).await;
    let svc = service(&e, &format!("{}/", server.uri()));
    let imp = svc.accounts().write().await.import_xtool(false).await.unwrap();
    let mut seen: Vec<String> = imp.warnings.clone();
    let clean = |text: &str| !text.contains(TEAM) && !text.contains(ISSUER);

    // Plans: an explicit identity the portal does not list is reported masked.
    let id = imp.identities[0].id.clone();
    let proj = project(e.tmp.path(), bundle, None);
    let plan = svc.plan_project(&proj, Distribution::Development, vec![], None).await.unwrap();
    seen.extend(plan.warnings.iter().map(|w| w.message.clone()));
    seen.extend(plan.mutations.iter().map(Mutation::summary));
    let p = svc.provisioner(None, None).await.unwrap();
    let mut req = plan.request.clone();
    req.identity = Some(IdentityId::new(TEAM, "FFFF"));
    let err = p.plan(&req).await.unwrap_err();
    seen.push(format!("{} {}", err.message(), serde_json::to_string(&err.to_data()).unwrap()));
    req.identity = Some(id.clone());
    fake.state.lock().unwrap().certificates.clear();
    let stale = p.plan(&req).await.unwrap();
    assert!(stale.warnings.iter().any(|w| w.message.contains("no longer lists")), "{:?}", stale.warnings);
    seen.extend(stale.warnings.iter().map(|w| w.message.clone()));

    // Identities: a missing key and a broken identity file.
    e.secrets.items.lock().unwrap().remove(&format!("signing-key/{}", imp.identities[0].sha1));
    let err = svc.identities().load_async(&id).await.unwrap_err();
    seen.push(format!("{} {:?}", err.message(), err.to_data()));
    let json = e.dirs.data.join("identities").join(TEAM).join(format!("{}.json", imp.identities[0].serial));
    std::fs::write(&json, b"{ broken").unwrap();
    let err = svc.identities().list(None).unwrap_err();
    seen.push(format!("{} {:?}", err.message(), err.to_data()));

    // Signing with a profile of another App ID: the profile's App ID is masked.
    let app = e.tmp.path().join("Hello.app");
    std::fs::create_dir_all(&app).unwrap();
    let mut info = plist::Dictionary::new();
    for (k, v) in
        [("CFBundleIdentifier", "com.other.app"), ("CFBundleExecutable", "Hello"), ("CFBundlePackageType", "APPL")]
    {
        info.insert(k.into(), plist::Value::String(v.into()));
    }
    plist::Value::Dictionary(info).to_file_xml(app.join("Info.plist")).unwrap();
    std::fs::write(app.join("Hello"), russet_macho::fixture::MachOBuilder::executable().build()).unwrap();
    let profile = e.tmp.path().join("hello.mobileprovision");
    std::fs::write(&profile, xtl_profile(bundle)).unwrap();
    let signing = russet_sign::testpki::development_identity(TEAM).load();
    let err = russet_sign::sign(russet_sign::SignRequest {
        bundle: &app,
        identity: &signing,
        per_bundle: std::collections::BTreeMap::from([(
            String::new(),
            russet_sign::BundleSigningInput::with_profile(profile),
        )]),
        options: russet_sign::SignOptions::default(),
    })
    .unwrap_err();
    assert_eq!(err.code(), codes::sign::PROFILE_MISMATCH);
    seen.push(format!("{} {}", err.message(), serde_json::to_string(&err.to_data()).unwrap()));

    assert!(seen.len() >= 6);
    for text in &seen {
        assert!(clean(text), "a team or issuer ID leaked: {text}");
    }
}
