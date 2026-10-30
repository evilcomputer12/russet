// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `account`, `identity` and `portal` handlers in-process: DTO mapping (team IDs masked, camelCase and
//! kebab-case on the wire) and their error paths. The domain logic is tested in russet-account.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use russet_account::{
    Account, AccountId, AccountKind, AccountService, AccountVerification, CertKind, IdentityId, ProbeResult,
    SigningIdentity,
};
use russet_core::dirs::RussetDirs;
use russet_core::error::codes;
use russet_core::net::Endpoints;
use russet_protocol::account as pa;
use russet_protocol::identity as pi;
use russet_protocol::portal as pp;
use russet_secrets::{BackendInfo, BackendKind, SecretBytes, SecretId, SecretKind, SecretMeta, SecretStore};
use russetd::handlers::{account, identity, portal, sign};

/// An in-memory secret store (russetd has no async-trait dependency, so the trait is implemented by hand).
#[derive(Default)]
struct MemorySecrets(Mutex<BTreeMap<String, Vec<u8>>>);

type Fut<'a, T> = Pin<Box<dyn Future<Output = russet_core::Result<T>> + Send + 'a>>;

impl SecretStore for MemorySecrets {
    fn put<'a, 'b, 'c, 'f>(&'a self, id: &'b SecretId, label: &'c str, v: SecretBytes) -> Fut<'f, SecretMeta>
    where
        'a: 'f,
        'b: 'f,
        'c: 'f,
    {
        Box::pin(async move {
            self.0.lock().unwrap().insert(id.to_string(), v.expose().to_vec());
            Ok(SecretMeta {
                id: id.clone(),
                label: label.into(),
                backend: BackendKind::Memory,
                created_at: time::OffsetDateTime::now_utc(),
                size_bytes: v.len(),
            })
        })
    }
    fn get<'a, 'b, 'f>(&'a self, id: &'b SecretId) -> Fut<'f, Option<SecretBytes>>
    where
        'a: 'f,
        'b: 'f,
    {
        Box::pin(async move { Ok(self.0.lock().unwrap().get(&id.to_string()).map(|v| SecretBytes::new(v.clone()))) })
    }
    fn delete<'a, 'b, 'f>(&'a self, id: &'b SecretId) -> Fut<'f, bool>
    where
        'a: 'f,
        'b: 'f,
    {
        Box::pin(async move { Ok(self.0.lock().unwrap().remove(&id.to_string()).is_some()) })
    }
    fn list<'a, 'f>(&'a self, _kind: Option<SecretKind>) -> Fut<'f, Vec<SecretMeta>>
    where
        'a: 'f,
    {
        Box::pin(async move { Ok(Vec::new()) })
    }
    fn backend(&self) -> BackendInfo {
        BackendInfo { kind: BackendKind::Memory, available: true, locked: false, reason: None }
    }
}

struct Home(std::path::PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn home(tag: &str) -> Home {
    let p = std::env::temp_dir().join(format!("russetd-w6-{tag}-{}-{:?}", std::process::id(), SystemTime::now()));
    std::fs::create_dir_all(&p).unwrap();
    Home(p)
}

fn service(h: &Home) -> AccountService {
    let u = |s: &str| russet_asc::Url::parse(s).unwrap();
    let endpoints = Endpoints {
        asc: u("http://127.0.0.1:9/"),
        gsa: u("http://127.0.0.1:9/"),
        tss: u("http://127.0.0.1:9/"),
        anisette_libs: u("http://127.0.0.1:9/"),
        github: u("http://127.0.0.1:9/"),
        flutter_storage: u("http://127.0.0.1:9/"),
    };
    let dirs = RussetDirs::under(&h.0.join("home"));
    AccountService::with_xtool_dir(&dirs, Arc::new(MemorySecrets::default()), endpoints, Some(h.0.join("no-xtool")))
        .unwrap()
}

fn account() -> Account {
    Account {
        id: AccountId::asc("2X9R4HXF34"),
        kind: AccountKind::AscTeamKey,
        label: "Team key".into(),
        key_id: Some("2X9R4HXF34".into()),
        issuer_id: Some("69a6de7d-1111-2222-3333-444455556666".into()),
        team_id: Some("ABCDE12345".into()),
        secret: "secret:asc-key/2X9R4HXF34".into(),
        imported_from: Some("xtool".into()),
        is_default: true,
    }
}

#[test]
fn accounts_go_on_the_wire_with_the_team_masked() {
    let dto = account::account_dto(&account());
    assert_eq!(dto.team_id.as_deref(), Some("*******345"));
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["kind"], "asc-team-key");
    assert_eq!(json["isDefault"], true);
    assert_eq!(json["importedFrom"], "xtool");
    assert!(json.get("issuerId").is_none(), "the issuer stays in the daemon");
    assert!(!json.to_string().contains("ABCDE12345"));

    let v = AccountVerification {
        account_id: AccountId::asc("2X9R4HXF34"),
        ok: true,
        team_id: Some("ABCDE12345".into()),
        certificates: ProbeResult::Ok,
        profiles: ProbeResult::Forbidden,
        builds: ProbeResult::Error("account.appleServiceUnavailable".into()),
    };
    let json = serde_json::to_value(account::verification_dto(&v)).unwrap();
    assert_eq!(
        json["roleProbe"],
        serde_json::json!({"certificates": "ok", "profiles": "forbidden", "builds": "error"})
    );
    assert_eq!(json["teamId"], "*******345");
}

#[test]
fn identities_go_on_the_wire_by_serial_without_the_team_or_subject() {
    let i = SigningIdentity {
        id: IdentityId::new("ABCDE12345", "4C5B2F"),
        team_id: "ABCDE12345".into(),
        kind: CertKind::Development,
        serial: "4C5B2F".into(),
        common_name: "Apple Development: Some Person (XYZ)".into(),
        not_after: SystemTime::now() + Duration::from_secs(86400),
        label: "development identity (imported from xtool)".into(),
        asc_certificate_id: None,
        sha1: "00".repeat(20),
        imported_from: Some("xtool".into()),
        cert_der: vec![1, 2, 3],
    };
    let json = serde_json::to_value(identity::identity_dto(&i)).unwrap();
    assert_eq!(json["kind"], "development");
    assert_eq!(json["expired"], false);
    assert_eq!(json["teamId"], "*******345");
    assert_eq!(json["id"], "4C5B2F", "the wire id is the certificate serial (it names no team)");
    assert!(!json.to_string().contains("Some Person"));
    assert!(!json.to_string().contains("ABCDE12345"), "{json}");
    assert!(json["notAfter"].as_str().unwrap().ends_with('Z'));
}

#[tokio::test]
async fn empty_stores_and_error_paths() {
    let h = home("acct");
    let svc = service(&h);
    let l = account::list(&svc).await.unwrap();
    assert!(l.accounts.is_empty() && l.default.is_none());
    let ids = identity::list(&svc, pi::IdentityListParams::default()).await.unwrap();
    assert!(ids.identities.is_empty());

    let e = account::verify(&svc, pa::AccountVerifyParams::default()).await.unwrap_err();
    assert_eq!(e.code(), codes::account::NO_ACCOUNT);
    assert!(e.hint().unwrap().contains("import --from-xtool"));

    let e = account::add_asc_key(
        &svc,
        pa::AccountAddAscKeyParams { key_id: "K".into(), issuer_id: None, p8_path: "relative.p8".into(), label: None },
    )
    .await
    .unwrap_err();
    assert_eq!(e.code(), codes::core::INVALID_ARGUMENT);

    let e = account::import_xtool(&svc, pa::AccountImportXtoolParams { dry_run: Some(true) }).await.unwrap_err();
    assert_eq!(e.code(), codes::core::NOT_FOUND, "no xtool login");
    assert_eq!(e.code().exit_code(), 3);

    let codes_of = [
        portal::certificates(&svc, pp::PortalListParams::default(), false).await.unwrap_err().code(),
        portal::bundle_ids(&svc, pp::PortalListParams::default()).await.unwrap_err().code(),
        portal::profiles(&svc, pp::PortalListParams::default()).await.unwrap_err().code(),
    ];
    assert!(codes_of.iter().all(|c| *c == codes::account::NO_ACCOUNT));
    let e = portal::devices(&svc, pp::PortalListParams::default(), false).await.unwrap_err();
    assert_eq!(e.code(), codes::account::NO_ACCOUNT);
}

// ---------------------------------------------------------------------------------------------------------------
// Handlers against an App Store Connect fake (axum, which russetd already depends on): add-key, list, verify,
// the portal listings with their reveal gate, and provision.plan/apply, with every result and error checked for
// the team ID (AGENTS.md §8: nothing the daemon sends names the team).

const TEAM: &str = "ABCDE12345";
const UDID: &str = "00008101-000A1B2C3D4E5F60";
const DEVICE_NAME: &str = "Someone's iPhone";
const PERSON: &str = "Apple Development: Some Person (QWERTY1234)";

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// A synthetic App Store Connect key: PKCS#8 for the P-256 scalar 0x22…22, assembled at run time (no key block is
/// committed).
fn p8_pem() -> String {
    let mut der = vec![
        0x30, 0x41, 0x02, 0x01, 0x00, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x04, 0x27, 0x30, 0x25, 0x02, 0x01, 0x01, 0x04, 0x20,
    ];
    der.extend([0x22u8; 32]);
    let label = ["PRIVATE", "KEY"].join(" ");
    let body = b64(&der);
    let lines: Vec<&str> = body.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

#[derive(Default)]
struct FakeAsc {
    requests: Mutex<Vec<(String, String)>>,
}

impl FakeAsc {
    fn respond(&self, method: &str, uri: &axum::http::Uri) -> (axum::http::StatusCode, String) {
        self.requests.lock().unwrap().push((method.to_owned(), uri.path().to_owned()));
        let ok = |v: serde_json::Value| (axum::http::StatusCode::OK, v.to_string());
        match uri.path() {
            "/v1/certificates" => ok(serde_json::json!({"data": [{"type": "certificates", "id": "C1", "attributes": {
                "certificateType": "DEVELOPMENT", "serialNumber": "7A7A7A7A7A7A7A7A", "displayName": PERSON,
                "expirationDate": "2027-10-10T00:00:00.000+0000"}}]})),
            "/v1/devices" => ok(serde_json::json!({"data": [{"type": "devices", "id": "D1", "attributes": {
                "udid": UDID, "name": DEVICE_NAME, "platform": "IOS", "status": "ENABLED", "deviceClass": "IPHONE"}}]})),
            // No bundle ID matches the project's, but one carries the team as its seed ID (team discovery).
            "/v1/bundleIds" => ok(serde_json::json!({"data": [{"type": "bundleIds", "id": "B9", "attributes": {
                "identifier": "com.someone.other", "platform": "IOS", "seedId": TEAM}}]})),
            "/v1/profiles" | "/v1/builds" | "/v1/apps" => ok(serde_json::json!({"data": []})),
            _ => (axum::http::StatusCode::NOT_FOUND, r#"{"errors":[{"status":"404","code":"NOT_FOUND"}]}"#.into()),
        }
    }
}

async fn fake_asc() -> (String, Arc<FakeAsc>) {
    let fake = Arc::new(FakeAsc::default());
    let f = fake.clone();
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let f = f.clone();
        async move {
            let (status, body) = f.respond(req.method().as_str(), req.uri());
            (status, [(axum::http::header::CONTENT_TYPE, "application/json")], body)
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/"), fake)
}

fn service_at(h: &Home, asc: &str) -> AccountService {
    let u = |s: &str| russet_asc::Url::parse(s).unwrap();
    let endpoints = Endpoints {
        asc: u(asc),
        gsa: u("http://127.0.0.1:9/"),
        tss: u("http://127.0.0.1:9/"),
        anisette_libs: u("http://127.0.0.1:9/"),
        github: u("http://127.0.0.1:9/"),
        flutter_storage: u("http://127.0.0.1:9/"),
    };
    let dirs = RussetDirs::under(&h.0.join("home"));
    AccountService::with_xtool_dir(&dirs, Arc::new(MemorySecrets::default()), endpoints, Some(h.0.join("no-xtool")))
        .unwrap()
}

fn no_team(what: &str, text: &str) {
    assert!(!text.contains(TEAM), "{what} names the team: {text}");
}

#[tokio::test]
async fn handlers_list_portal_records_with_the_reveal_gate_and_never_name_the_team() {
    use russet_core::rpc::ClientKind;
    use russet_protocol::provision as pv;
    use russetd::handlers::provision;

    let h = home("portal");
    let (asc, fake) = fake_asc().await;
    let svc = service_at(&h, &asc);

    // account.addAscKey: the key is accepted and the team discovered (a bundle ID's seed ID).
    let p8 = h.0.join("AuthKey_2X9R4HXF34.p8");
    std::fs::write(&p8, p8_pem()).unwrap();
    let added = account::add_asc_key(
        &svc,
        pa::AccountAddAscKeyParams {
            key_id: "2X9R4HXF34".into(),
            issuer_id: Some("69a6de7d-1111-2222-3333-444455556666".into()),
            p8_path: p8.display().to_string(),
            label: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(added.team_id.as_deref(), Some("*******345"));
    let mut wire = vec![("addAscKey", serde_json::to_string(&added).unwrap())];
    wire.push(("list", serde_json::to_string(&account::list(&svc).await.unwrap()).unwrap()));
    let v = account::verify(&svc, pa::AccountVerifyParams::default()).await.unwrap();
    assert!(v.ok);
    wire.push(("verify", serde_json::to_string(&v).unwrap()));

    // portal.devices.list: UDIDs masked and names omitted, except for an ide connection that asks.
    let ask = pp::PortalListParams { reveal: Some(true), ..Default::default() };
    let plain = pp::PortalListParams::default();
    for (params, kind, revealed) in [
        (&plain, ClientKind::Cli, false),
        (&ask, ClientKind::Cli, false),
        (&ask, ClientKind::Mcp, false),
        (&plain, ClientKind::Ide, false),
        (&ask, ClientKind::Ide, true),
    ] {
        let reveal = portal::reveal_allowed(params, kind);
        assert_eq!(reveal, revealed, "{kind:?} {:?}", params.reveal);
        let devices = portal::devices(&svc, params.clone(), reveal).await.unwrap();
        let text = serde_json::to_string(&devices).unwrap();
        assert_eq!(text.contains(UDID), revealed, "{kind:?}: {text}");
        assert_eq!(text.contains(DEVICE_NAME), revealed, "{kind:?}: {text}");
        assert!(text.contains("0000…5F60") != revealed, "{text}");
        let certs = portal::certificates(&svc, params.clone(), reveal).await.unwrap();
        let text = serde_json::to_string(&certs).unwrap();
        assert_eq!(text.contains("Some Person"), revealed, "certificate display names name a person: {text}");
        assert!(text.contains("7A7A7A7A7A7A7A7A"));
        wire.push(("portal", text));
    }
    wire.push(("bundleIds", serde_json::to_string(&portal::bundle_ids(&svc, plain.clone()).await.unwrap()).unwrap()));
    wire.push(("profiles", serde_json::to_string(&portal::profiles(&svc, plain.clone()).await.unwrap()).unwrap()));

    // provision.plan (GET-only) lists the account changes a new bundle needs; provision.apply refuses them.
    let project = h.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("russet.toml"), "schema = 1\n[app]\nbundle-id = \"dev.russet.test.probe\"\n").unwrap();
    let plan = provision::plan(
        &svc,
        pv::ProvisionPlanParams {
            project: project.display().to_string(),
            distribution: None,
            devices: Some(vec![UDID.into()]),
            account_id: None,
        },
    )
    .await
    .unwrap();
    let kinds: Vec<pv::MutationKind> = plan.mutations.iter().map(|m| m.kind).collect();
    assert!(kinds.contains(&pv::MutationKind::CreateCertificate), "no local identity: {kinds:?}");
    assert!(kinds.contains(&pv::MutationKind::RegisterBundleId));
    assert!(kinds.contains(&pv::MutationKind::CreateProfile));
    assert_eq!(plan.team_id, "*******345");
    let text = serde_json::to_string(&plan).unwrap();
    assert!(!text.contains(UDID), "{text}");
    wire.push(("plan", text));
    // A mutating plan is refused without an approval (the handler's approval wiring is covered in
    // russet-account and the approval tests; here we check the domain refusal leaks no team).
    let err = svc.apply_plan(&plan.plan_id, &russet_account::ApprovalRef::default()).await.unwrap_err();
    assert_eq!(err.code(), codes::perm::APPROVAL_REQUIRED);
    wire.push(("apply", format!("{} {}", err.message(), serde_json::to_string(&err.to_data()).unwrap())));

    // Errors on the way out: an identity the store does not have, and a path error naming the team's directory.
    let err = sign::run(
        &svc,
        russet_protocol::sign::SignRunParams {
            path: h.0.display().to_string(),
            distribution: russet_protocol::sign::Distribution::Development,
            identity_id: Some(format!("{TEAM}/FFFF")),
            profiles: BTreeMap::new(),
            entitlements: BTreeMap::new(),
            merge_policy: None,
            output: None,
            get_task_allow: None,
            device: None,
            no_fix: Vec::new(),
            lint: None,
            no_lint: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), codes::sign::NO_IDENTITY);
    let err = account::redacted(Some(&svc), Err::<(), _>(err)).await.unwrap_err();
    wire.push(("sign.run", format!("{} {}", err.message(), serde_json::to_string(&err.to_data()).unwrap())));
    let raw = russet_core::Error::new(codes::core::IO, format!("cannot read /x/identities/{TEAM}/1.json"))
        .with_details(serde_json::json!({"team": TEAM}));
    let err = account::redacted(Some(&svc), Err::<(), _>(raw)).await.unwrap_err();
    wire.push(("redacted", format!("{} {}", err.message(), serde_json::to_string(&err.to_data()).unwrap())));

    for (what, text) in &wire {
        no_team(what, text);
    }
    let reqs = fake.requests.lock().unwrap().clone();
    assert!(!reqs.is_empty() && reqs.iter().all(|(m, _)| m == "GET"), "Wave 1 reads only: {reqs:?}");
}

/// The other W6 DTOs, built from values that carry the team: none of them names it on the wire.
#[test]
fn every_dto_masks_the_team() {
    use russet_account::{BundleProvision, PlanId, PlannedProfile, ProvisionOutcome, ProvisionPlan, ProvisionRequest};
    use russetd::handlers::provision;

    let id = IdentityId::new(TEAM, "4C5B2F");
    let plan = ProvisionPlan {
        plan_id: PlanId("pln_01J00000000000000000000000".into()),
        digest: [7; 32],
        team_id: TEAM.into(),
        mutations: vec![russet_account::Mutation::CreateCertificate { kind: CertKind::Development }],
        warnings: vec![russet_account::PlanWarning { message: format!("the portal no longer lists {id}") }],
        account_id: AccountId::asc("2X9R4HXF34"),
        distribution: russet_sign::Distribution::Development,
        identity: Some(id.clone()),
        profiles: vec![PlannedProfile {
            rel_path: String::new(),
            bundle_id: "XTL-69A6DE7D.com.example.hello".into(),
            profile_id: "P1".into(),
            uuid: "11111111-2222-4333-8444-555555555555".into(),
            name: "XTL com.example.hello".into(),
            expires: "2027-10-10T00:00:00Z".into(),
            content: Vec::new(),
        }],
        request: ProvisionRequest {
            bundles: Vec::new(),
            distribution: russet_sign::Distribution::Development,
            devices: Vec::new(),
            identity: Some(id.clone()),
        },
    };
    let dto = provision::plan_dto(&plan);
    assert_eq!(dto.identity_id.as_deref(), Some("4C5B2F"));
    let outcome = ProvisionOutcome {
        identity: Some(id.clone()),
        bundles: vec![BundleProvision {
            rel_path: String::new(),
            bundle_id: "XTL-69A6DE7D.com.example.hello".into(),
            profile_uuid: "11111111-2222-4333-8444-555555555555".into(),
            profile_path: "/home/u/.local/share/russet/profiles/11111111-2222-4333-8444-555555555555.mobileprovision"
                .into(),
        }],
    };
    let report = russet_sign::SignReport {
        output: "/tmp/out/Probe.app".into(),
        bundles: vec![russet_sign::SignedBundle {
            rel_path: String::new(),
            bundle_id: "dev.russet.test.probe".into(),
            kind: russet_sign::BundleKind::App,
            cdhash: "00".repeat(20),
            team_id: TEAM.into(),
            profile_uuid: "11111111-2222-4333-8444-555555555555".into(),
            entitlements_xml: format!("<string>{TEAM}.dev.russet.test.probe</string>"),
            notes: Vec::new(),
        }],
        fixes_applied: Vec::new(),
        lints: Vec::new(),
    };
    let verified = russet_sign::VerifyReport {
        ok: true,
        path: "/tmp/out/Probe.app".into(),
        bundles: vec![russet_sign::VerifiedBundle {
            rel_path: String::new(),
            bundle_id: Some("dev.russet.test.probe".into()),
            identifier: Some("dev.russet.test.probe".into()),
            cdhash: Some("00".repeat(20)),
            team_id: Some(TEAM.into()),
            profile_uuid: None,
            signed: true,
        }],
        findings: Vec::new(),
    };
    let texts = [
        serde_json::to_string(&dto).unwrap(),
        serde_json::to_string(&provision::outcome_dto(&outcome)).unwrap(),
        serde_json::to_string(&sign::sign_report_dto(&report)).unwrap(),
        serde_json::to_string(&sign::verify_dto(&verified)).unwrap(),
        serde_json::to_string(&account::account_dto(&account())).unwrap(),
    ];
    for t in &texts {
        no_team("dto", t);
    }
    assert!(texts[2].contains("*******345") && texts[3].contains("*******345"));
}
