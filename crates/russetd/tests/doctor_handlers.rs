// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `doctor.run` on the real development box (env-gated; PLAN §4.3 acceptance): every check of the doctor seam
//! over this host's toolchains, the secret backend `RUSSET_SECRETS_BACKEND` selects, and the device backend of
//! the environment. Fixture-based doctor tests are in `toolchain_handlers.rs`.
//!
//! ```sh
//! RUSSET_TEST_HOST=1 RUSSET_DEVICE_BACKEND=none cargo test -p russetd --test doctor_handlers -- --nocapture
//! ```

use std::sync::Arc;

use russet_config::ConfigService;
use russet_core::dirs::RussetDirs;
use russet_protocol::doctor as d;
use russetd::handlers::doctor::{self as dh, DoctorServices};

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
    let mut secrets_cfg = russet_secrets::SecretsConfig::from_env().expect("secrets config");
    secrets_cfg.russet_home = Some(home.clone());
    let secrets = russet_secrets::open_default(&dirs, &secrets_cfg).expect("secrets");
    let toolchains = Arc::new(russet_toolchain::Toolchains::load(&dirs).expect("toolchains"));
    let services = DoctorServices::from_env(&dirs, config, secrets, toolchains);
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
