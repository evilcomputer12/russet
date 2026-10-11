// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Passphrase mode works with `DBUS_SESSION_BUS_ADDRESS` unset (SA-01, PLAN §4.3): the parent test re-runs this
//! test binary with the session bus removed from the environment, and the child stores and reads a secret through
//! `open_default` in `auto` mode, which must choose the passphrase store and never reach D-Bus.

use std::process::Command;

use russet_core::dirs::RussetDirs;
use russet_secrets::{BackendKind, SecretBytes, SecretId, SecretKind, SecretsConfig, open_default};

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
    assert!(!cfg.session_bus);
    let home = cfg.russet_home.clone().expect("RUSSET_HOME");
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
