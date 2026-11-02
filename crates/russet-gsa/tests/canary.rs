// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The log canary of the Apple ID path (AGENTS.md §8, ARCHITECTURE §17): a whole sign-in with two-factor, the
//! Xcode token and a developer-services read runs at TRACE with every log line captured, and none of the
//! password, Apple ID, code, tokens, `adsid` or anisette values may appear. One test per binary, with a global
//! subscriber, so the capture cannot miss events.

#![allow(clippy::unwrap_used)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::{Arc, Mutex};

use base64::Engine as _;

use russet_anisette::fake::AnisetteFake;
use russet_gsa::fake::{GsaFake, GsaFakeConfig};
use russet_gsa::{ClientProfile, Ds2Session, GsaClient, LoginState, Qh65b2, TwoFactorVia};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_secret_reaches_the_log() {
    let logs = Capture::default();
    let sub = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::set_global_default(sub).unwrap();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cfg = GsaFakeConfig::default();
    let fake = GsaFake::start(cfg.clone()).await;
    let gsa = Arc::new(GsaClient::new(
        reqwest::Client::new(),
        fake.gsa_url(),
        Arc::new(AnisetteFake::new()),
        ClientProfile::builtin(),
    ));
    let LoginState::NeedsTwoFactor(p) = gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap() else {
        panic!("two-factor expected");
    };
    gsa.request_code(&p, TwoFactorVia::TrustedDevice).await.unwrap();
    let wrong = gsa.submit_code(&p, TwoFactorVia::TrustedDevice, "135791").await.unwrap_err();
    tracing::warn!(error = %wrong.display_line(), "sign-in step failed");
    gsa.submit_code(&p, TwoFactorVia::TrustedDevice, &cfg.code).await.unwrap();
    let LoginState::Authenticated(s) = gsa.login(&cfg.apple_id, cfg.password.as_bytes()).await.unwrap() else {
        panic!("signed in expected");
    };
    let token = gsa.xcode_token(&s).await.unwrap();
    tracing::info!(?s, ?token, "session ready");
    let qh = Qh65b2::new(fake.ds2_url(), Arc::new(Ds2Session::new(gsa.clone(), token))).unwrap();
    let teams = qh.list_teams().await.unwrap();
    assert_eq!(teams.len(), 1);

    let text = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
    assert!(text.contains("gsa: srp complete") && text.contains("session ready"), "the capture works: {text}");
    // The debug lines name the stage, the Status and the header names, never their values.
    for line in [
        "gsa: request",
        "gsa: response",
        "stage=\"init\"",
        "stage=\"complete\"",
        "ec=0",
        "platform=\"windows\"",
        "X-Mme-Client-Info",
        "stage=\"validate\"",
        "ds2: response",
        "action=\"listTeams\"",
        "kind=\"xml-plist\"",
    ] {
        assert!(text.contains(line), "missing `{line}` in the debug log: {text}");
    }
    let identity_token = base64::engine::general_purpose::STANDARD.encode(format!("{}:fake-idms-token", cfg.adsid));
    for secret in [
        cfg.password.as_str(),
        cfg.apple_id.as_str(),
        cfg.code.as_str(),
        cfg.adsid.as_str(),
        cfg.team_id.as_str(),
        "canary-",
        "canary.",
        "fake-idms-token",
        "fake-xcode-token",
        "ZmFrZS1vdHAt",
        "cnVzc2V0LWZha2UtbWFjaGluZS1pZA",
        identity_token.as_str(),
        "Windows;6.2",
        "CFNetwork/808",
    ] {
        assert!(!text.contains(secret), "`{secret}` leaked into the log");
    }
}
