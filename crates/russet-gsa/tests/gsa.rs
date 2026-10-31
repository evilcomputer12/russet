// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The mocked GrandSlam suite (W22 acceptance: `GsaFake`, `AnisetteFake`): SRP login, both two-factor flows, the
//! Xcode token, developerservices2 teams and `v1` reads. The log canary is `tests/canary.rs`.

#![allow(clippy::unwrap_used)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::Arc;

use russet_anisette::fake::AnisetteFake;
use russet_asc::ds2::Ds2Auth;
use russet_core::error::codes;
use russet_gsa::fake::{GsaFake, GsaFakeConfig};
use russet_gsa::{ClientProfile, Ds2Session, GsaClient, LoginState, Qh65b2, SecondFactor, TeamTier, TwoFactorVia};

fn client(fake: &GsaFake) -> Arc<GsaClient> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Arc::new(GsaClient::new(
        reqwest::Client::new(),
        fake.gsa_url(),
        Arc::new(AnisetteFake::new()),
        ClientProfile::builtin(),
    ))
}

fn assert_no_canary(text: &str, cfg: &GsaFakeConfig) {
    for secret in [
        cfg.password.as_str(),
        cfg.apple_id.as_str(),
        cfg.code.as_str(),
        "canary",
        "fake-idms-token",
        "fake-xcode-token",
    ] {
        assert!(!text.contains(secret), "`{secret}` leaked into: {text}");
    }
}

#[tokio::test]
async fn paid_login_with_a_trusted_device_code_reaches_developer_services() {
    let cfg = GsaFakeConfig::default();
    let fake = GsaFake::start(cfg.clone()).await;
    let gsa = client(&fake);

    let LoginState::NeedsTwoFactor(pending) =
        gsa.login(&cfg.apple_id.to_uppercase(), cfg.password.as_bytes()).await.unwrap()
    else {
        panic!("two-factor expected");
    };
    assert_eq!(pending.kind, SecondFactor::TrustedDevice);
    gsa.request_code(&pending, TwoFactorVia::TrustedDevice).await.unwrap();
    assert_eq!(fake.stats().pushes, 1);

    let wrong = gsa.submit_code(&pending, TwoFactorVia::TrustedDevice, "000000").await.unwrap_err();
    assert_eq!(wrong.code(), codes::gsa::BAD_CODE);
    let malformed = gsa.submit_code(&pending, TwoFactorVia::TrustedDevice, "12a").await.unwrap_err();
    assert_eq!(malformed.code(), codes::gsa::BAD_CODE);
    gsa.submit_code(&pending, TwoFactorVia::TrustedDevice, &cfg.code).await.unwrap();

    let LoginState::Authenticated(session) = gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap() else {
        panic!("signed in expected after two-factor");
    };
    assert_eq!(session.dsid, cfg.dsid.to_string());
    let token = gsa.xcode_token(&session).await.unwrap();
    assert!(token.fresh(time::OffsetDateTime::now_utc(), time::Duration::minutes(5)));

    let creds = Arc::new(Ds2Session::new(gsa.clone(), token));
    let qh = Qh65b2::new(fake.ds2_url(), creds.clone()).unwrap();
    let teams = qh.list_teams().await.unwrap();
    assert_eq!(teams.len(), 1);
    assert_eq!(teams[0].team_id, cfg.team_id);
    assert_eq!(teams[0].tier(), TeamTier::Paid);

    let auth = Arc::new(Ds2Auth::new(fake.ds2_url(), cfg.team_id.clone(), creds));
    let asc = russet_asc::AscClient::new(auth).unwrap();
    let certs: serde_json::Value = asc.get_json("/v1/certificates?limit=1").await.unwrap();
    assert!(certs["data"].is_array());
    assert_eq!(fake.stats().v1_requests, 1);

    let stats = fake.stats();
    assert_eq!((stats.inits, stats.completes, stats.tokens_issued, stats.team_lists), (2, 2, 1, 1));

    // The password never crossed the wire (the log canary is tests/canary.rs).
    for r in fake.received().await {
        let body = String::from_utf8_lossy(&r.body);
        assert!(!body.contains(&cfg.password), "the password crossed the wire in {}", r.url.path());
    }
}

#[tokio::test]
async fn sms_only_accounts_list_numbers_and_verify_by_text() {
    let cfg = GsaFakeConfig { two_factor: Some(SecondFactor::Sms), protocol: "s2k_fo", ..Default::default() };
    let fake = GsaFake::start(cfg.clone()).await;
    let gsa = client(&fake);
    let LoginState::NeedsTwoFactor(pending) = gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap() else {
        panic!("two-factor expected");
    };
    assert_eq!(pending.kind, SecondFactor::Sms);
    let numbers = gsa.trusted_numbers(&pending).await.unwrap();
    assert_eq!(numbers.len(), 1);
    assert!(numbers[0].obfuscated.ends_with("12") && numbers[0].obfuscated.contains('•'));
    gsa.request_code(&pending, TwoFactorVia::Sms { phone_id: 1 }).await.unwrap();
    assert_eq!(fake.stats().sms_sent, vec![1]);
    let wrong = gsa.submit_code(&pending, TwoFactorVia::Sms { phone_id: 1 }, "999999").await.unwrap_err();
    assert_eq!(wrong.code(), codes::gsa::BAD_CODE);
    gsa.submit_code(&pending, TwoFactorVia::Sms { phone_id: 1 }, &cfg.code).await.unwrap();
    assert!(matches!(gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap(), LoginState::Authenticated(_)));
}

#[tokio::test]
async fn a_free_team_without_two_factor_signs_in_directly() {
    let cfg = GsaFakeConfig { two_factor: None, paid: false, ..Default::default() };
    let fake = GsaFake::start(cfg.clone()).await;
    let gsa = client(&fake);
    let LoginState::Authenticated(session) = gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap() else {
        panic!("signed in expected");
    };
    let token = gsa.xcode_token(&session).await.unwrap();
    let qh = Qh65b2::new(fake.ds2_url(), Arc::new(Ds2Session::new(gsa.clone(), token))).unwrap();
    assert_eq!(qh.list_teams().await.unwrap()[0].tier(), TeamTier::Free);
}

#[tokio::test]
async fn a_wrong_password_is_bad_credentials_and_says_nothing_secret() {
    let cfg = GsaFakeConfig::default();
    let fake = GsaFake::start(cfg.clone()).await;
    let gsa = client(&fake);
    let e = gsa.login(&cfg.apple_id, b"not the password").await.unwrap_err();
    assert_eq!(e.code(), codes::gsa::BAD_CREDENTIALS);
    let text = format!("{e:?} {} {:?}", e.display_line(), e.hint());
    assert_no_canary(&text, &cfg);
    let unknown = gsa.login("someone.else@example.invalid", cfg.password.as_bytes()).await.unwrap_err();
    assert_eq!(unknown.code(), codes::gsa::BAD_CREDENTIALS);
}

#[tokio::test]
async fn a_stale_token_is_refused_by_developer_services() {
    let cfg = GsaFakeConfig { two_factor: None, token_seconds: 1, ..Default::default() };
    let fake = GsaFake::start(cfg.clone()).await;
    let gsa = client(&fake);
    let LoginState::Authenticated(session) = gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap() else {
        panic!("signed in expected");
    };
    let token = gsa.xcode_token(&session).await.unwrap();
    let qh = Qh65b2::new(fake.ds2_url(), Arc::new(Ds2Session::new(gsa.clone(), token))).unwrap();
    // Within 30 s of expiry the session refuses to send the token at all.
    let e = qh.list_teams().await.unwrap_err();
    assert_eq!(e.code(), codes::gsa::SESSION_EXPIRED);
}

#[tokio::test]
async fn the_request_shape_matches_what_grandslam_accepts() {
    let cfg = GsaFakeConfig::default();
    let fake = GsaFake::start(cfg.clone()).await;
    let gsa = client(&fake);
    let LoginState::NeedsTwoFactor(p) = gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap() else {
        panic!("two-factor expected");
    };
    gsa.request_code(&p, TwoFactorVia::TrustedDevice).await.unwrap();
    let windows = "<PC> <Windows;6.2(0,0);9200> <com.apple.AuthKit/1 (com.apple.akd/1.0)>";
    let received = fake.received().await;
    let header =
        |r: &wiremock::Request, k: &str| r.headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    let gs: Vec<_> =
        received.iter().filter(|r| r.method.as_str() == "POST" && r.url.path() == "/grandslam/GsService2").collect();
    assert_eq!(gs.len(), 2, "init and complete");
    for (r, op) in gs.iter().zip(["init", "complete"]) {
        assert_eq!(header(r, "user-agent"), "akd/1.0 CFNetwork/808.1.4");
        assert_eq!(header(r, "x-mme-client-info"), windows);
        assert_eq!(header(r, "content-type"), "text/x-xml-plist");
        let v: plist::Value = plist::from_bytes(&r.body).unwrap();
        let root = v.as_dictionary().unwrap();
        assert_eq!(root["Header"].as_dictionary().unwrap()["Version"].as_string(), Some("1.0.1"));
        let req = root["Request"].as_dictionary().unwrap();
        assert_eq!(req["o"].as_string(), Some(op));
        let cpd = req["cpd"].as_dictionary().unwrap();
        assert_eq!(cpd["X-MMe-Client-Info"].as_string(), Some(windows));
        for k in [
            "bootstrap",
            "icscrec",
            "pbe",
            "prkgen",
            "svct",
            "loc",
            "X-Apple-I-MD",
            "X-Apple-I-MD-M",
            "X-Apple-I-MD-RINFO",
            "X-Apple-I-MD-LU",
            "X-Mme-Device-Id",
            "X-Apple-I-Client-Time",
            "X-Apple-Locale",
            "X-Apple-I-TimeZone",
        ] {
            assert!(cpd.contains_key(k), "cpd lacks {k}");
        }
        if op == "init" {
            let ps: Vec<_> = req["ps"].as_array().unwrap().iter().filter_map(plist::Value::as_string).collect();
            assert_eq!(ps, ["s2k", "s2k_fo"]);
        }
    }
    let push = received.iter().find(|r| r.url.path() == "/auth/verify/trusteddevice").unwrap();
    assert_eq!(header(push, "user-agent"), "akd/1.0 CFNetwork/808.1.4");
    assert_eq!(header(push, "x-mme-client-info"), windows);
}

#[tokio::test]
async fn a_macos_identity_over_android_anisette_is_refused_at_init_like_grandslam_does() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cfg = GsaFakeConfig::default();
    let fake = GsaFake::start(cfg.clone()).await;
    let mut profile = ClientProfile::builtin();
    profile.anisette_client_info_override =
        Some("<MacBookPro18,3> <macOS;27.0;26A5378j> <com.apple.AuthKit/1 (com.apple.akd/1.0)>".into());
    let gsa = GsaClient::new(reqwest::Client::new(), fake.gsa_url(), Arc::new(AnisetteFake::new()), profile);
    let e = gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap_err();
    assert_eq!(e.code(), codes::gsa::IDENTITY_REJECTED);
    assert!(e.message().contains("at init (-80045)"), "{}", e.message());
    let stats = fake.stats();
    assert_eq!((stats.identity_rejections, stats.completes), (1, 0));
}

#[tokio::test]
async fn a_bare_503_from_grandslam_is_retried_once() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cfg = GsaFakeConfig { two_factor: None, busy_inits: 1, ..Default::default() };
    let fake = GsaFake::start(cfg.clone()).await;
    let gsa =
        GsaClient::new(reqwest::Client::new(), fake.gsa_url(), Arc::new(AnisetteFake::new()), ClientProfile::builtin())
            .with_retry_delay(std::time::Duration::ZERO);
    assert!(matches!(gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap(), LoginState::Authenticated(_)));
    assert_eq!((fake.stats().inits, fake.stats().busy_answers), (2, 1));

    let cfg = GsaFakeConfig { two_factor: None, busy_inits: 2, ..Default::default() };
    let fake = GsaFake::start(cfg.clone()).await;
    let gsa =
        GsaClient::new(reqwest::Client::new(), fake.gsa_url(), Arc::new(AnisetteFake::new()), ClientProfile::builtin())
            .with_retry_delay(std::time::Duration::ZERO);
    let e = gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap_err();
    assert_eq!(e.code(), codes::gsa::IDENTITY_REJECTED);
    assert!(e.message().contains("HTTP 503 at init"), "{}", e.message());
}

#[tokio::test]
async fn developer_services_answers_are_read_whatever_their_encoding() {
    use russet_gsa::fake::Ds2Body;
    for shape in [Ds2Body::Xml, Ds2Body::Binary, Ds2Body::Gzip, Ds2Body::BomXml, Ds2Body::Json, Ds2Body::Html] {
        let cfg = GsaFakeConfig { two_factor: None, ds2_body: shape, ..Default::default() };
        let fake = GsaFake::start(cfg.clone()).await;
        let gsa = client(&fake);
        let LoginState::Authenticated(session) = gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap()
        else {
            panic!("signed in expected");
        };
        let token = gsa.xcode_token(&session).await.unwrap();
        let qh = Qh65b2::new(fake.ds2_url(), Arc::new(Ds2Session::new(gsa.clone(), token))).unwrap();
        let teams = qh.list_teams().await;
        if shape == Ds2Body::Html {
            let e = teams.unwrap_err();
            assert_eq!(e.code(), codes::gsa::PROTOCOL);
            assert!(e.message().contains("listTeams") && e.message().contains("html"), "{}", e.message());
        } else {
            let teams = teams.unwrap_or_else(|e| panic!("{shape:?}: {}", e.message()));
            assert_eq!((teams.len(), teams[0].tier()), (1, TeamTier::Paid), "{shape:?}");
        }
    }
}
