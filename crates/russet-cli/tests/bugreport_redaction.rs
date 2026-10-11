// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet bugreport` (core §12, CORE-13): the zip holds the build, the daemon's answers (or why there are
//! none), the daemon logs and the configuration, and the redaction canary appears nowhere in it. Nor do the
//! identifiers a report must not carry when it is attached to an issue (ARCHITECTURE §12.1): UDIDs, Apple IDs,
//! DSIDs, team, key and issuer IDs, signing-identity and device names, and the home directory.
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
    assert!(!entries.contains(&"config/accounts.toml"));
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

/// Identifiers of the canary account and device (synthetic, never real ones).
const UDID: &str = "00008101-001C4A7A2C4A7001";
const LEGACY_UDID: &str = "c4a7c4a7c4a7c4a7c4a7c4a7c4a7c4a7c4a7c4a7";
const APPLE_ID: &str = "canary.person@example.com";
const TEAM_ID: &str = "C4NARY7EAM";
const KEY_ID: &str = "C4NARYKEY9";
const ISSUER_ID: &str = "c4a7a7e1-0000-4000-8000-00000000c4a7";
const DSID: &str = "40404040404";
const PERSON: &str = "Canary Person";
const DEVICE: &str = "Canary Phone";

#[test]
fn the_bug_report_masks_identifiers() {
    let home = Home::new("bugreport-ids");
    let config = home.root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("accounts.toml"),
        format!(
            "schema = 1\ndefault = \"asc:{KEY_ID}\"\n\n[[account]]\nid = \"asc:{KEY_ID}\"\nkind = \"asc-team-key\"\n\
             label = \"Team key\"\nkey-id = \"{KEY_ID}\"\nissuer-id = \"{ISSUER_ID}\"\nteam-id = \"{TEAM_ID}\"\n\
             secret = \"secret:asc-key/{KEY_ID}\"\n\n[[account]]\nid = \"appleid:{DSID}\"\nkind = \"apple-id\"\n\
             apple-id = \"{APPLE_ID}\"\nteam-id = \"{TEAM_ID}\"\n"
        ),
    )
    .unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!(
            "schema = 1\n[devices.aliases]\nphone = \"{UDID}\"\n[device.ddi]\ndir = \"{}/ddi\"\n",
            home.root.display()
        ),
    )
    .unwrap();
    std::fs::write(config.join("provision-policy.toml"), format!("schema = 1\nallowed-teams = [\"{TEAM_ID}\"]\n"))
        .unwrap();
    let logs = home.root.join("state/logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("russetd.2026-10-11.log"),
        format!(
            "2026-10-11T09:00:00.000Z  INFO russet_device: attached udid={UDID} deviceName=\"{DEVICE}\"\n\
             2026-10-11T09:00:01.000Z  INFO russet_device: legacy device {LEGACY_UDID} paired\n\
             2026-10-11T09:00:02.000Z  INFO russet_sign: signing with Apple Development: {PERSON} ({TEAM_ID}) \
             for {TEAM_ID}.com.example.probe\n\
             2026-10-11T09:00:03.000Z  WARN russet_account: account {APPLE_ID} (appleid:{DSID}) needs a code\n\
             2026-10-11T09:00:04.000Z  INFO russet_asc: GET /v1/profiles 200 key_id={KEY_ID} issuer_id={ISSUER_ID}\n\
             2026-10-11T09:00:05.000Z  INFO russet_account: imported AuthKey_{KEY_ID}.p8\n"
        ),
    )
    .unwrap();
    let out_path = home.root.join("ids.zip");
    // The home directory is shown as `~`: let the temporary home stand for it.
    let out = home
        .cmd()
        .env("HOME", &home.root)
        .args(["--json", "bugreport", "--out", out_path.to_str().unwrap()])
        .output()
        .expect("russet runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let entries: Vec<&str> = v["entries"].as_array().unwrap().iter().filter_map(|e| e.as_str()).collect();
    assert!(entries.contains(&"config/accounts.summary.json") && !entries.contains(&"config/accounts.toml"));
    let bytes = std::fs::read(&out_path).expect("zip");
    let text = String::from_utf8_lossy(&bytes);
    for (what, id) in [
        ("UDID", UDID),
        ("legacy UDID", LEGACY_UDID),
        ("Apple ID", APPLE_ID),
        ("team ID", TEAM_ID),
        ("key ID", KEY_ID),
        ("issuer ID", ISSUER_ID),
        ("DSID", DSID),
        ("signing identity", PERSON),
        ("device name", DEVICE),
    ] {
        assert!(!text.contains(id), "the {what} is in the bug report:\n{text}");
    }
    assert!(!text.contains(home.root.to_str().unwrap()), "the home directory is in the bug report:\n{text}");
    assert!(text.contains("dir = \"~/ddi\""), "{text}");
    // The report still says which device and team a line is about, and what kinds of account exist.
    assert!(text.contains("udid=<udid-1>") && text.contains("<team-id-1>.com.example.probe"), "{text}");
    assert!(text.contains("\"kind\": \"asc-team-key\"") && text.contains("\"kind\": \"apple-id\""), "{text}");
    assert!(text.contains("AuthKey_<key-id-1>.p8") && text.contains("(appleid:<dsid-1>)"), "{text}");
}
