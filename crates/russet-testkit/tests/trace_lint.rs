// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! QA-22 security tests for the trace redaction safety net (QA §3.5.3): the lint flags leaked identifiers and
//! Apple-file payloads, the recorder redacts sensitive keys, and an elided service never stores its payload.

use russet_device::Udid;
use russet_device::io::ServiceConnector;
use russet_testkit::rdt::{Device, Dir, Header, Record, Redaction, Trace, VERSION, base64_encode};
use russet_testkit::{RecordingConnector, Scenario, ScriptedConnector, redaction};
use tokio::io::AsyncReadExt;

const UDID: &str = "00000000-0000000000000001";
const MACHO: &[u8] = &[0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0x00, 0x00, 0x01];

fn header(scenario: &str) -> Header {
    Header {
        rdt: VERSION,
        scenario: scenario.into(),
        recorded_at: "2026-10-11T00:00:00Z".into(),
        russet: "0.1.0".into(),
        idevice: "0.1.68".into(),
        host_os: "linux".into(),
        device: Device {
            product_type: "iPhone13,2".into(),
            product_version: "27.0".into(),
            build_version: "24A437".into(),
            csm: "txm".into(),
            udid: UDID.into(),
            transport: "usb".into(),
        },
        redaction: Redaction::default(),
    }
}

/// SEC-lint-01: a trace carrying real identifiers and an Apple-file payload is flagged.
#[test]
fn the_lint_catches_planted_identifiers() {
    let mut trace = Trace::new(header("planted"));
    trace.records.push(Record::Call {
        t: 0.0,
        api: "lockdown.get_value".into(),
        args: serde_json::json!({}),
        result: Some(serde_json::json!({
            "UniqueDeviceID": "00008101-0000000000000000",
            "WiFiAddress": "ac:de:48:00:11:22",
            "AppleId": "victim@gmail.com",
            "HostPrivateKey": "-----BEGIN EC PRIVATE KEY-----",
        })),
        error: None,
    });
    trace.records.push(Record::Data { conn: 1, t: 1.0, dir: Dir::D2c, b64: base64_encode(MACHO) });

    let findings = redaction::lint(&trace);
    for kind in ["udid", "mac", "email", "private-key", "apple-file"] {
        assert!(findings.iter().any(|f| f.kind == kind), "{kind} not flagged in {findings:?}");
    }
}

/// SEC-redact-01: the recorder replaces sensitive keys and the result lints clean.
#[test]
fn redaction_makes_a_result_clean() {
    let mut v = serde_json::json!({
        "UniqueDeviceID": "00008101-0000000000000000",
        "SerialNumber": "F2LXXXXXXXXX",
        "WiFiAddress": "ac:de:48:00:11:22",
        "DeviceName": "Martin's iPhone",
        "ProductType": "iPhone13,2",
    });
    redaction::redact_value(&mut v);
    let mut trace = Trace::new(header("redacted"));
    trace.records.push(Record::Call {
        t: 0.0,
        api: "x".into(),
        args: serde_json::json!({}),
        result: Some(v.clone()),
        error: None,
    });
    assert!(redaction::lint(&trace).is_empty(), "redacted result still trips the lint: {:?}", redaction::lint(&trace));
    assert_eq!(v["ProductType"], serde_json::json!("iPhone13,2"), "non-sensitive keys are kept");
    assert_ne!(v["UniqueDeviceID"], serde_json::json!("00008101-0000000000000000"), "the UDID is gone");
}

/// SEC-elide-01: an elided service's payload becomes an `Elided` record, not bytes — and without elision the
/// same Mach-O payload would be caught by the lint.
#[tokio::test]
async fn an_elided_service_stores_no_payload() {
    let scenario = format!(
        r#"
        schema = 1
        name = "elide"
        [[device]]
        udid = "{UDID}"
        [[service]]
        name = "com.apple.dt.fetchsymbols"
        via = "lockdown"
        steps = [{{ send-b64 = "{b64}" }}, {{ close = true }}]
        [[service]]
        name = "com.example.blob"
        via = "lockdown"
        steps = [{{ send-b64 = "{b64}" }}, {{ close = true }}]
        "#,
        b64 = base64_encode(MACHO),
    );

    // The default-elided fetchsymbols service: no payload is stored.
    let scripted = ScriptedConnector::new(Scenario::from_toml(&scenario).expect("scenario"));
    let rec = RecordingConnector::new(scripted, header("elide"));
    read_all(&rec, "com.apple.dt.fetchsymbols").await;
    let trace = rec.trace();
    assert!(
        trace.records.iter().any(|r| matches!(r, Record::Elided { reason, .. } if reason == "apple-file:symbols")),
        "the elided service produced no Elided record: {:?}",
        trace.records
    );
    assert!(
        !trace.records.iter().any(|r| matches!(r, Record::Data { b64, .. } if b64 == &base64_encode(MACHO))),
        "the elided service stored the payload bytes",
    );
    assert!(redaction::lint(&trace).is_empty(), "the elided trace is clean: {:?}", redaction::lint(&trace));

    // A non-elided service recording the same Mach-O: the lint catches it (proving elision is the control).
    let scripted = ScriptedConnector::new(Scenario::from_toml(&scenario).expect("scenario"));
    let rec = RecordingConnector::new(scripted, header("blob"));
    read_all(&rec, "com.example.blob").await;
    let trace = rec.trace();
    assert!(
        redaction::lint(&trace).iter().any(|f| f.kind == "apple-file"),
        "a stored Mach-O payload must trip the lint",
    );
}

async fn read_all<C: ServiceConnector>(rec: &RecordingConnector<C>, service: &str) {
    let mut s = rec.lockdown_service(&Udid(UDID.into()), service).await.expect("stream");
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.expect("read to EOF");
    assert_eq!(buf, MACHO, "the client still sees the real bytes");
    drop(s); // Drop records the Elided/close records.
}
