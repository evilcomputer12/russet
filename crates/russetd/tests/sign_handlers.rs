// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `sign` handlers in-process: path validation, `.app` and `.ipa` reading, the object-shaped results of
//! `sign.lint` and `sign.inspect`, and DTO mapping. Signing itself is tested in russet-sign and russet-account.

#![allow(clippy::unwrap_used)]

use std::time::SystemTime;

use russet_core::error::codes;
use russet_protocol::sign as p;
use russetd::handlers::sign;

struct Dir(std::path::PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(tag: &str) -> Dir {
    let p = std::env::temp_dir().join(format!("russetd-w6-{tag}-{}-{:?}", std::process::id(), SystemTime::now()));
    std::fs::create_dir_all(&p).unwrap();
    Dir(p)
}

/// A bundle whose executable is not a Mach-O file: readable, but never valid.
fn bogus_app(d: &Dir) -> std::path::PathBuf {
    let app = d.0.join("Bogus.app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("Info.plist"),
        "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>dev.russet.test.bogus</string><key>CFBundleExecutable</key><string>Bogus</string></dict></plist>",
    )
    .unwrap();
    std::fs::write(app.join("Bogus"), b"#!/bin/sh\n").unwrap();
    app
}

fn params(path: &std::path::Path) -> p::SignPathParams {
    p::SignPathParams { path: path.display().to_string(), profile: None, device: None }
}

#[tokio::test]
async fn reading_handlers_report_objects() {
    let d = dir("sign");
    let app = bogus_app(&d);
    let v = sign::verify(params(&app)).await.unwrap();
    assert!(!v.ok);
    assert!(v.findings.iter().any(|f| f.code == "L-SIG-000" && f.severity == p::Severity::Error));
    let json = serde_json::to_value(&v).unwrap();
    assert!(json["findings"][0]["severity"].is_string());

    let l = sign::lint(params(&app)).await.unwrap();
    assert!(l.findings.iter().any(|f| f.code == "L-SIG-000" && f.severity == p::Severity::Info));
    let json = serde_json::to_value(&l).unwrap();
    assert!(json.get("findings").is_some(), "sign.lint answers an object");

    let i = sign::inspect(params(&app)).await.unwrap();
    assert!(i.signatures.is_empty(), "no Mach-O, no signature");
}

#[tokio::test]
async fn bad_paths_and_inputs_are_errors() {
    let d = dir("sign-bad");
    assert_eq!(
        sign::verify(params(std::path::Path::new("relative.app"))).await.unwrap_err().code(),
        codes::core::INVALID_ARGUMENT
    );
    assert_eq!(sign::lint(params(&d.0.join("missing.app"))).await.unwrap_err().code(), codes::core::NOT_FOUND);
    let not_ipa = d.0.join("notes.txt");
    std::fs::write(&not_ipa, b"plain text").unwrap();
    assert_eq!(sign::inspect(params(&not_ipa)).await.unwrap_err().code(), codes::sign::BUNDLE_INVALID);
}

#[test]
fn findings_and_signatures_map_to_the_wire() {
    let f = russet_sign::LintFinding {
        code: "L-MACHO-001".into(),
        message: "m".into(),
        severity: russet_sign::Severity::Warning,
        path: Some("Probe".into()),
    };
    let dto = sign::finding_dto(&f);
    assert_eq!(dto.severity, p::Severity::Warning);
    assert_eq!(serde_json::to_value(&dto).unwrap()["path"], "Probe");
    assert_eq!(sign::lint_profile(p::LintProfile::AppStore), russet_sign::LintProfile::AppStore);
}
