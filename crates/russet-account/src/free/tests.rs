// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The mocked Apple ID suite of `russet-account` (W22): sign-in through `GsaFake` with `AnisetteFake` or local ADI
//! on `FakeAdi`, the account and sealed session it leaves, and the DS2 portal of the signed-in team.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use russet_anisette::fake::{AnisetteFake, FakeAdi};
use russet_core::dirs::RussetDirs;
use russet_core::error::codes;
use russet_core::net::Endpoints;
use russet_gsa::fake::{GsaFake, GsaFakeConfig};
use russet_gsa::{SecondFactor, TwoFactorVia};
use russet_secrets::memory::MemoryStore;
use russet_secrets::{SecretBytes, SecretStore};

use super::*;
use crate::store::{AccountKind, AccountStore};

struct Home {
    _tmp: tempfile::TempDir,
    dirs: RussetDirs,
    secrets: Arc<dyn SecretStore>,
    accounts: tokio::sync::RwLock<AccountStore>,
}

fn home() -> Home {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tmp = tempfile::tempdir().unwrap();
    let dirs = RussetDirs::under(&tmp.path().join("home"));
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let accounts = tokio::sync::RwLock::new(AccountStore::load(&dirs, secrets.clone()).unwrap());
    Home { _tmp: tmp, dirs, secrets, accounts }
}

fn service(h: &Home, fake: &GsaFake) -> AppleIds {
    let endpoints = Endpoints { gsa: fake.gsa_url(), ..Endpoints::default() };
    AppleIds::new(&h.dirs, h.secrets.clone(), endpoints).with_ds2_base(fake.ds2_url())
}

#[tokio::test]
async fn a_paid_apple_id_signs_in_with_a_trusted_device_code() {
    let h = home();
    let cfg = GsaFakeConfig::default();
    let fake = GsaFake::start(cfg.clone()).await;
    let svc = Arc::new(service(&h, &fake).with_anisette(Arc::new(AnisetteFake::new())));

    let BeginOutcome::TwoFactor { session_id, info } = svc
        .begin(
            &h.accounts,
            &cfg.apple_id,
            SecretBytes::new(cfg.password.clone().into_bytes()),
            AnisetteSetting::Local,
            None,
        )
        .await
        .unwrap()
    else {
        panic!("two-factor expected");
    };
    assert!(info.trusted_device && info.pushed);
    assert_eq!(info.phone_numbers.len(), 1);
    assert_eq!(fake.stats().pushes, 1);

    let wrong = svc.submit_code(&h.accounts, &session_id, "111111").await.unwrap_err();
    assert_eq!(wrong.code(), codes::gsa::BAD_CODE);
    let account = svc.submit_code(&h.accounts, &session_id, &cfg.code).await.unwrap();
    assert_eq!(account.kind, AccountKind::AppleId);
    assert_eq!(account.id.0, format!("appleid:{}", cfg.dsid));
    assert_eq!(account.team_id.as_deref(), Some(cfg.team_id.as_str()));
    assert!(account.label.starts_with("Apple ID (ca…@ex…)"), "{}", account.label);
    // The session is single-use.
    let again = svc.submit_code(&h.accounts, &session_id, &cfg.code).await.unwrap_err();
    assert_eq!(again.code(), codes::gsa::LOGIN_EXPIRED);

    let store = h.accounts.read().await;
    assert_eq!(store.list().len(), 1);
    let entry = store.apple_id_entry(&account.id).unwrap();
    assert_eq!(entry.team_tier.as_deref(), Some("paid"));
    let text = std::fs::read_to_string(store.path()).unwrap();
    assert!(text.contains("team-tier = \"paid\"") && text.contains("anisette = \"local\""), "{text}");
    let sealed = h.secrets.get(&entry.secret_id()).await.unwrap().unwrap();
    let sealed = String::from_utf8_lossy(sealed.expose()).into_owned();
    assert!(!sealed.contains(&cfg.password), "the password is never stored");
    assert!(!format!("{account:?} {entry:?}").contains(&cfg.apple_id));
    drop(store);

    // The account's portal reads through developerservices2 with the sealed session.
    AppleIds::register(svc.clone());
    let portal = h.accounts.read().await.portal(&account.id, None).await.unwrap();
    assert_eq!(portal.team_id(), cfg.team_id);
    assert!(portal.list_certificates(None).await.unwrap().is_empty());
    assert_eq!(fake.stats().v1_requests, 1);

    // Removing it with wipeSecrets deletes the sealed session.
    h.accounts.write().await.remove(&account.id, true).await.unwrap();
    assert!(h.secrets.get(&entry.secret_id()).await.unwrap().is_none());
}

#[tokio::test]
async fn sms_only_sign_in_and_unknown_sessions() {
    let h = home();
    let cfg = GsaFakeConfig { two_factor: Some(SecondFactor::Sms), paid: false, ..Default::default() };
    let fake = GsaFake::start(cfg.clone()).await;
    let svc = service(&h, &fake).with_anisette(Arc::new(AnisetteFake::new()));
    let BeginOutcome::TwoFactor { session_id, info } = svc
        .begin(
            &h.accounts,
            &cfg.apple_id,
            SecretBytes::new(cfg.password.clone().into_bytes()),
            AnisetteSetting::Local,
            Some("Mine".into()),
        )
        .await
        .unwrap()
    else {
        panic!("two-factor expected");
    };
    assert!(!info.trusted_device && !info.pushed);
    svc.request_code(&session_id, TwoFactorVia::Sms { phone_id: 1 }).await.unwrap();
    assert_eq!(fake.stats().sms_sent, vec![1]);
    let account = svc.submit_code(&h.accounts, &session_id, &cfg.code).await.unwrap();
    assert_eq!(account.label, "Mine");
    let entry = h.accounts.read().await.apple_id_entry(&account.id).unwrap();
    assert_eq!(entry.team_tier.as_deref(), Some("free"));

    let e = svc.request_code("no-such-session", TwoFactorVia::TrustedDevice).await.unwrap_err();
    assert_eq!(e.code(), codes::gsa::LOGIN_EXPIRED);
    let e = svc
        .begin(&h.accounts, &cfg.apple_id, SecretBytes::new(b"wrong".to_vec()), AnisetteSetting::Local, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), codes::gsa::BAD_CREDENTIALS);
}

/// Writes synthetic stand-ins for the two libraries with a trust-on-first-use record, as a fetch would.
fn install_fake_libraries(data: &std::path::Path) -> Option<()> {
    let abi = russet_anisette::Abi::host()?;
    let dir = russet_anisette::libs::libs_root(data).join("0.0.1-fake").join(abi.dir_name());
    std::fs::create_dir_all(&dir).unwrap();
    let mut tofu = serde_json::Map::new();
    for lib in russet_anisette::libs::LIBRARIES {
        let bytes = format!("synthetic {lib}").into_bytes();
        std::fs::write(dir.join(lib), &bytes).unwrap();
        let hex: String =
            ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect();
        tofu.insert(lib.into(), hex.into());
    }
    std::fs::write(dir.join("tofu.json"), serde_json::to_vec(&tofu).unwrap()).unwrap();
    Some(())
}

#[tokio::test]
async fn local_anisette_provisions_once_and_is_reported() {
    let h = home();
    let cfg = GsaFakeConfig { two_factor: None, ..Default::default() };
    let fake = GsaFake::start(cfg.clone()).await;
    let svc = service(&h, &fake).with_adi_backend(Arc::new(FakeAdi::new()));
    let missing = svc
        .begin(
            &h.accounts,
            &cfg.apple_id,
            SecretBytes::new(cfg.password.clone().into_bytes()),
            AnisetteSetting::Local,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(missing.code(), codes::anisette::LIBS_MISSING);
    assert!(!svc.anisette_status().await.libraries_present);
    if install_fake_libraries(&h.dirs.data).is_none() {
        return; // no ADI ABI on this CPU
    }

    let BeginOutcome::Done { account, .. } = svc
        .begin(
            &h.accounts,
            &cfg.apple_id,
            SecretBytes::new(cfg.password.clone().into_bytes()),
            AnisetteSetting::Local,
            None,
        )
        .await
        .unwrap()
    else {
        panic!("signed in expected");
    };
    assert_eq!(account.kind, AccountKind::AppleId);
    assert_eq!(fake.stats().provisionings, 1);
    let status = svc.anisette_status().await;
    assert!(status.libraries_present && status.provisioned);
    assert_eq!((status.apk_version.as_deref(), status.pinned), (Some("0.0.1-fake"), Some(false)));
    // A second sign-in reuses the provisioning.
    svc.begin(
        &h.accounts,
        &cfg.apple_id,
        SecretBytes::new(cfg.password.clone().into_bytes()),
        AnisetteSetting::Local,
        None,
    )
    .await
    .unwrap();
    assert_eq!(fake.stats().provisionings, 1);
}

#[test]
fn apple_ids_are_masked() {
    assert_eq!(masked_apple_id("someone@example.com"), "so…@ex…");
    assert_eq!(masked_apple_id("x"), "x…");
}
