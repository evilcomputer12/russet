// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `doctor.run`: the report of core §11 and the per-call class (`read`, `fix` → `system`) [D 66, D 98].

use russet_protocol::doctor::{
    DoctorBuild, DoctorCategory, DoctorCheck, DoctorFix, DoctorHost, DoctorReport, DoctorStatus, DoctorSummary,
};
use russet_protocol::{ClientKind, ConnInfo, JobId, LeaseId, Ownership, Permission, registry};
use serde_json::json;

struct NoOwnership;

impl Ownership for NoOwnership {
    fn owns_job(&self, _conn: &str, _job: &JobId) -> bool {
        false
    }
    fn owns_lease(&self, _conn: &str, _lease: &LeaseId) -> bool {
        false
    }
    fn project_bundle_ids(&self, _conn: &str) -> Vec<String> {
        Vec::new()
    }
}

#[test]
fn fix_makes_doctor_a_system_call() {
    let m = registry().iter().find(|m| m.name == "doctor.run").copied().expect("doctor.run");
    let conn = ConnInfo {
        id: "c-1".into(),
        kind: ClientKind::Mcp,
        name: "agent".into(),
        interactive: false,
        shared_lease: None,
    };
    let class = |p: serde_json::Value| (m.permission_of)(&p, &conn, &NoOwnership).map_err(|e| e.code());
    assert_eq!(class(json!({})), Ok(Permission::Read));
    assert_eq!(class(json!({"fix": false, "online": true})), Ok(Permission::Read));
    assert_eq!(class(json!({"fix": true})), Ok(Permission::System));
}

#[test]
fn the_report_has_the_core_11_shape() {
    let report = DoctorReport {
        russet: DoctorBuild { version: "0.1.0".into(), build_id: "0.1.0+gabc".into() },
        host: DoctorHost { os: "linux".into(), distro: Some("cachyos".into()), arch: "x86_64".into(), wsl: false },
        checks: vec![DoctorCheck {
            id: "usbmux.available".into(),
            category: DoctorCategory::Usbmux,
            title: "usbmuxd".into(),
            status: DoctorStatus::Skip,
            message: "device backend is none".into(),
            detail: None,
            hint: None,
            fix: Some(DoctorFix::Command { command: "sudo systemctl enable --now usbmuxd".into() }),
            duration_ms: 1,
        }],
        summary: DoctorSummary { pass: 0, warn: 0, fail: 0, skip: 1 },
    };
    let v = serde_json::to_value(&report).expect("ser");
    assert_eq!(v["russet"]["buildId"], "0.1.0+gabc");
    assert_eq!(v["checks"][0]["status"], "skip");
    assert_eq!(v["checks"][0]["durationMs"], 1);
    assert_eq!(v["checks"][0]["fix"], json!({"kind": "command", "command": "sudo systemctl enable --now usbmuxd"}));
    assert_eq!(v["summary"], json!({"pass": 0, "warn": 0, "fail": 0, "skip": 1}));
    let back: DoctorReport = serde_json::from_value(v).expect("round trip");
    assert_eq!(back, report);
}
