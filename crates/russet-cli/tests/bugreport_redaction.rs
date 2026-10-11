// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet bugreport` (core §12, CORE-13): the zip holds the build, the daemon's answers (or why there are
//! none), the daemon logs and the configuration, and the redaction canary appears nowhere in it.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use support::Home;

const CANARY: &str = "RuSsEtBuGrEpOrTcAnArY77";

#[test]
fn the_bug_report_has_no_canary() {
    let home = Home::new("bugreport");
    let logs = home.root.join("state/logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("russetd.2026-10-11.log"),
        format!("2026-10-11T09:00:00.000Z  INFO x: Authorization: Bearer {CANARY}\nline token={CANARY}\n"),
    )
    .unwrap();
    let config = home.root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.toml"), format!("schema = 1\n[mcp.http]\ntoken = \"{CANARY}\"\n")).unwrap();
    let out_path = home.root.join("report.zip");
    let v = home.json(&["bugreport", "--out", out_path.to_str().unwrap()]);
    let entries: Vec<&str> = v["entries"].as_array().unwrap().iter().filter_map(|e| e.as_str()).collect();
    for expected in ["russet.json", "daemon-status.json", "logs/russetd.2026-10-11.log", "config/config.toml"] {
        assert!(entries.contains(&expected), "{expected} missing in {entries:?}");
    }
    let bytes = std::fs::read(&out_path).expect("zip");
    assert!(bytes.starts_with(b"PK\x03\x04"));
    assert!(!bytes.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()), "the canary is in the bug report");
    assert!(
        bytes.windows(b"token = \"***\"".len()).any(|w| w == b"token = \"***\""),
        "config lines are kept, redacted"
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&out_path).unwrap().permissions().mode() & 0o777, 0o600);
}
