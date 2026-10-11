// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Passphrase mode works with `DBUS_SESSION_BUS_ADDRESS` unset (SA-01, PLAN §4.3): the parent test re-runs this
//! test binary with the session bus removed from the environment, and the child stores and reads a secret through
//! `open_default` in `auto` mode, which must choose the passphrase store and never reach D-Bus.

use std::process::Command;

use russet_core::dirs::RussetDirs;
use russet_secrets::{BackendKind, SecretBytes, SecretId, SecretKind, SecretsConfig, SecretsEnv, open_default};

const CHILD_MARKER: &str = "RUSSET_SECRETS_NO_BUS_CHILD";

#[test]
fn passphrase_mode_works_without_a_session_bus() {
    let exe = std::env::current_exe().expect("test binary");
    let home = tempfile::tempdir().expect("home");
    let out = Command::new(exe)
        .args(["--exact", "child_stores_a_secret_without_dbus", "--include-ignored", "--nocapture"])
        .env(CHILD_MARKER, "1")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env_remove("XDG_RUNTIME_DIR")
        .env_remove("RUSSET_SECRETS_BACKEND")
        .env("RUSSET_SECRETS_PASSPHRASE", "headless passphrase")
        .env("RUSSET_HOME", home.path())
        .output()
        .expect("run the child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("1 passed"), "the child test did not run:\n{stdout}");
}

#[test]
#[ignore = "run by passphrase_mode_works_without_a_session_bus in a child process"]
fn child_stores_a_secret_without_dbus() {
    if std::env::var_os(CHILD_MARKER).is_none() {
        return;
    }
    assert!(std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none());
    let cfg = SecretsConfig::from_env().expect("config");
    let env = SecretsEnv::from_env();
    assert!(!env.session_bus);
    let home = env.russet_home.clone().expect("RUSSET_HOME");
    let store = open_default(&RussetDirs::under(&home), &cfg).expect("open");
    assert_eq!(store.backend().kind, BackendKind::PassphraseFile);
    let rt = tokio::runtime::Builder::new_current_thread().build().expect("runtime");
    rt.block_on(async {
        let id = SecretId { kind: SecretKind::AscKey, name: "K".into() };
        store.put(&id, "k", SecretBytes::new(vec![9; 4096])).await.expect("put");
        assert_eq!(store.get(&id).await.expect("get").expect("value").expose(), &[9; 4096][..]);
        assert!(!store.status().await.locked);
    });
}

/// A session bus that leads nowhere (no Secret Service): `auto` falls back to the passphrase store on first use.
#[test]
fn auto_falls_back_to_the_passphrase_when_the_keyring_does_not_answer() {
    let exe = std::env::current_exe().expect("test binary");
    let home = tempfile::tempdir().expect("home");
    let out = Command::new(exe)
        .args(["--exact", "child_falls_back_to_the_passphrase", "--include-ignored", "--nocapture"])
        .env(CHILD_MARKER, "1")
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", home.path().join("no-bus").display()))
        .env_remove("RUSSET_SECRETS_BACKEND")
        .env("RUSSET_SECRETS_PASSPHRASE", "fallback passphrase")
        .env("RUSSET_HOME", home.path())
        .output()
        .expect("run the child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("1 passed"), "the child test did not run:\n{stdout}");
    let store_json = std::fs::read_to_string(home.path().join("data/secrets/store.json")).expect("store.json");
    assert!(store_json.contains("\"passphrase\""), "{store_json}");
}

#[test]
#[ignore = "run by auto_falls_back_to_the_passphrase_when_the_keyring_does_not_answer in a child process"]
fn child_falls_back_to_the_passphrase() {
    if std::env::var_os(CHILD_MARKER).is_none() {
        return;
    }
    let cfg = SecretsConfig::from_env().expect("config");
    let env = SecretsEnv::from_env();
    assert!(env.session_bus, "a (broken) session bus is configured");
    let home = env.russet_home.clone().expect("RUSSET_HOME");
    let store = open_default(&RussetDirs::under(&home), &cfg).expect("open");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).build().expect("runtime");
    rt.block_on(async {
        let id = SecretId { kind: SecretKind::GithubToken, name: "me".into() };
        store.put(&id, "gh", SecretBytes::new(b"token".to_vec())).await.expect("put");
        assert_eq!(store.get(&id).await.expect("get").expect("value").expose(), b"token");
        assert_eq!(store.status().await.kind, BackendKind::PassphraseFile);
    });
}
