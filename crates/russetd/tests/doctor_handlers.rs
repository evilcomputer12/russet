// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `doctor.run` with a service that did not start, and on the real development box (env-gated; PLAN §4.3
//! acceptance): every check of the doctor seam over this host's toolchains, the secret backend
//! `RUSSET_SECRETS_BACKEND` selects, and the device backend of the environment. Fixture-based doctor tests are in
//! `toolchain_handlers.rs`.
//!
//! ```sh
//! RUSSET_TEST_HOST=1 RUSSET_DEVICE_BACKEND=none cargo test -p russetd --test doctor_handlers -- --nocapture
//! ```

use std::sync::Arc;

use russet_config::ConfigService;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, codes};
use russet_protocol::doctor as d;
use russetd::handlers::doctor::{self as dh, DoctorServices};

/// A secret store that did not open (sealed by another backend, say) fails `secrets.backend` with its error and
/// hint; the other checks still run, and `doctor.run` itself succeeds.
#[tokio::test]
async fn a_service_that_did_not_start_fails_its_checks_only() {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let home = std::env::temp_dir().join(format!("russet-w3-doctor-down-{}-{nanos}", std::process::id()));
    let dirs = RussetDirs::under(&home);
    let config = Arc::new(ConfigService::with_env(&dirs, Default::default()).expect("config"));
    let locked = Error::new(codes::secrets::LOCKED, "data/secrets was sealed by the passphrase backend, not keyring")
        .with_hint("Unset RUSSET_SECRETS_BACKEND.");
    let toolchains = Arc::new(russet_toolchain::Toolchains::new(
        dirs.clone(),
        Arc::new(russet_toolchain::pins::PinManifest::empty()),
    ));
    let services = DoctorServices::from_env(&dirs, Ok(config), Err(locked), Ok(toolchains));
    let mut registry = russet_doctor::Registry::new();
    for c in dh::checks() {
        registry.register(c);
    }
    let only = d::DoctorRunParams { checks: Some(vec!["secrets.".into(), "config.".into()]), ..Default::default() };
    let report = dh::run(&registry, &services, only).await;
    let _ = std::fs::remove_dir_all(&home);
    let report = report.expect("doctor.run succeeds with a service down");
    let secrets = report.checks.iter().find(|c| c.id == "secrets.backend").expect("secrets.backend");
    assert_eq!(secrets.status, d::DoctorStatus::Fail, "{secrets:?}");
    assert!(secrets.message.starts_with("the secret store did not start: secrets.locked"), "{}", secrets.message);
    assert_eq!(secrets.hint.as_deref(), Some("Unset RUSSET_SECRETS_BACKEND."));
    let global = report.checks.iter().find(|c| c.id == "config.global").expect("config.global");
    assert_eq!(global.status, d::DoctorStatus::Pass, "{global:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_on_the_dev_box() {
    if std::env::var_os("RUSSET_TEST_HOST").is_none_or(|v| v != "1") {
        eprintln!("skipped: set RUSSET_TEST_HOST=1 to run every doctor check on this host");
        return;
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let home = std::env::temp_dir().join(format!("russet-w3-doctor-{}-{nanos}", std::process::id()));
    let dirs = RussetDirs::under(&home);
    let config = Arc::new(ConfigService::load(&dirs).expect("config"));
    let secrets_cfg = russet_secrets::SecretsConfig::from_env().expect("secrets config");
    let secrets_env =
        russet_secrets::SecretsEnv { russet_home: Some(home.clone()), ..russet_secrets::SecretsEnv::from_env() };
    let secrets = russet_secrets::open_with(&dirs, &secrets_cfg, &secrets_env).expect("secrets");
    let toolchains = Arc::new(russet_toolchain::Toolchains::load(&dirs).expect("toolchains"));
    let services = DoctorServices::from_env(&dirs, Ok(config), Ok(secrets), Ok(toolchains));
    let report = dh::run(&russetd::doctor_registry::registry(), &services, d::DoctorRunParams::default()).await;
    let _ = std::fs::remove_dir_all(&home);
    let report = report.expect("doctor");
    eprintln!("{}", serde_json::to_string_pretty(&report).expect("json"));
    for c in &report.checks {
        if c.id.starts_with("toolchain.") && c.id != "toolchain.xtool" || c.id == "secrets.backend" {
            assert_eq!(c.status, d::DoctorStatus::Pass, "{}: {}", c.id, c.message);
        }
    }
    let mux = report.checks.iter().find(|c| c.id == "usbmux.available").expect("usbmux.available");
    assert!(matches!(mux.status, d::DoctorStatus::Pass | d::DoctorStatus::Skip), "{mux:?}");
    assert_eq!(report.summary.fail, 0);
}
