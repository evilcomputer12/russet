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
use russetd::handlers::{account, identity, portal};

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
fn identities_go_on_the_wire_without_the_subject() {
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
    assert!(!json.to_string().contains("Some Person"));
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
        portal::certificates(&svc, pp::PortalListParams::default()).await.unwrap_err().code(),
        portal::bundle_ids(&svc, pp::PortalListParams::default()).await.unwrap_err().code(),
        portal::profiles(&svc, pp::PortalListParams::default()).await.unwrap_err().code(),
    ];
    assert!(codes_of.iter().all(|c| *c == codes::account::NO_ACCOUNT));
    let e = portal::devices(&svc, pp::PortalListParams::default(), false).await.unwrap_err();
    assert_eq!(e.code(), codes::account::NO_ACCOUNT);
}
