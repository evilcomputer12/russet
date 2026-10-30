// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Redaction policy `default-v1` and the trace lint (QA §3.5.3).
//!
//! The recorder never stores personal identifiers or Apple-file payloads. [`redact_value`] maps the identifiers
//! a facade result may carry (UDIDs, serials, addresses, names, team IDs, Apple-ID emails) to stable synthetic
//! pseudonyms, and the recorder elides Apple-file payloads ([`crate::rdt::Record::Elided`]).
//!
//! [`lint`] is the safety net: it fails on anything that looks like a real identifier or an Apple file in a
//! trace, so a trace can never be committed with a real UDID, MAC, email, private key or Mach-O/IMG4/DMG blob
//! in it. `cargo xtask traces lint` runs the same checks over committed `*.rdt.jsonl` files (DC-13).
//!
//! The lint runs over the JSON structure, not the raw text: text identifiers are matched in string *values*
//! (and `note` decodings), while `data` payloads are base64-decoded and checked for file magic and size. This
//! keeps base64 blobs from raising false hex/MAC matches.

use serde_json::Value;

use crate::rdt::{Record, Trace};

/// The redaction policy this testkit applies and asserts.
pub const POLICY: &str = "default-v1";

/// The largest a single `data` record's decoded payload may be (QA §3.5.3).
pub const MAX_DATA_BYTES: usize = 1024 * 1024;

/// One thing the lint found wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The 1-based line in the JSON Lines trace (the header is line 1).
    pub line: usize,
    /// The rule that fired (`udid`, `mac`, `email`, `private-key`, `apple-file`, `oversized-data`).
    pub kind: &'static str,
    /// What was found (already shortened; never the full secret).
    pub detail: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}: {}", self.line, self.kind, self.detail)
    }
}

/// Lints a parsed trace. The header is line 1; record `i` is line `i + 2`.
pub fn lint(trace: &Trace) -> Vec<Finding> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::to_value(&trace.header) {
        lint_value(1, &v, &mut out);
    }
    for (i, rec) in trace.records.iter().enumerate() {
        let line = i + 2;
        match rec {
            Record::Data { b64, .. } => lint_data(line, b64, &mut out),
            Record::Elided { .. } => {} // elided payloads are already gone; the hash and length are safe.
            other => {
                if let Ok(v) = serde_json::to_value(other) {
                    lint_value(line, &v, &mut out);
                }
            }
        }
    }
    out
}

/// Lints already-decompressed JSON Lines bytes (what `cargo xtask traces lint` reads from a plain trace).
pub fn lint_jsonl(bytes: &[u8]) -> std::result::Result<Vec<Finding>, String> {
    let trace = Trace::from_jsonl(bytes).map_err(|e| e.message().to_owned())?;
    Ok(lint(&trace))
}

/// Recursively scans a JSON value's strings for identifier shapes.
fn lint_value(line: usize, v: &Value, out: &mut Vec<Finding>) {
    match v {
        Value::String(s) => scan_text(line, s, out),
        Value::Array(a) => a.iter().for_each(|v| lint_value(line, v, out)),
        Value::Object(o) => o.values().for_each(|v| lint_value(line, v, out)),
        _ => {}
    }
}

/// Checks a `data` payload: base64-decode, then file magic and size.
fn lint_data(line: usize, b64: &str, out: &mut Vec<Finding>) {
    let Ok(bytes) = crate::scenario::base64_decode(b64) else {
        return;
    };
    if bytes.len() > MAX_DATA_BYTES {
        out.push(Finding {
            line,
            kind: "oversized-data",
            detail: format!("{} bytes (max {MAX_DATA_BYTES})", bytes.len()),
        });
    }
    if let Some(magic) = apple_file_magic(&bytes) {
        out.push(Finding { line, kind: "apple-file", detail: format!("payload begins with {magic}") });
    }
}

/// The file kind a payload's magic bytes name, if it is one the recorder must elide (QA §3.5.3).
pub fn apple_file_magic(bytes: &[u8]) -> Option<&'static str> {
    let starts = |m: &[u8]| bytes.len() >= m.len() && &bytes[..m.len()] == m;
    let at = |off: usize, m: &[u8]| bytes.len() >= off + m.len() && &bytes[off..off + m.len()] == m;
    if starts(&[0xca, 0xfe, 0xba, 0xbe]) || starts(&[0xfe, 0xed, 0xfa, 0xce]) || starts(&[0xfe, 0xed, 0xfa, 0xcf]) {
        return Some("a Mach-O binary");
    }
    if starts(&[0xcf, 0xfa, 0xed, 0xfe]) {
        return Some("a Mach-O binary");
    }
    if starts(b"IMG4") || at(1, b"IMG4") {
        return Some("an IMG4 firmware image");
    }
    if at(0x101, b"typ4") || starts(b"xar!") {
        return Some("a xar/IMG4 archive");
    }
    if starts(b"BOMStore") {
        return Some("a BOMStore");
    }
    // A DMG "koly" trailer sits at the end; flag a payload whose last 512 bytes contain it.
    let tail = &bytes[bytes.len().saturating_sub(512)..];
    if tail.windows(4).any(|w| w == b"koly") {
        return Some("a DMG (koly trailer)");
    }
    None
}

/// Scans a single string for UDID, MAC, email and private-key shapes.
fn scan_text(line: usize, s: &str, out: &mut Vec<Finding>) {
    if let Some(d) = find_udid(s) {
        out.push(Finding { line, kind: "udid", detail: d });
    }
    if let Some(d) = find_mac(s) {
        out.push(Finding { line, kind: "mac", detail: d });
    }
    if let Some(d) = find_email(s) {
        out.push(Finding { line, kind: "email", detail: d });
    }
    if s.contains("PRIVATE KEY-----") || s.contains("-----BEGIN RSA") || s.contains("-----BEGIN EC") {
        out.push(Finding { line, kind: "private-key", detail: "a PEM private key".into() });
    }
}

/// A real-device UDID shape: `XXXXXXXX-XXXXXXXXXXXXXXXX` with a non-zero chip id (the fake allowlist is chip id
/// `00000000`), or a 40-hex-character run. Returns a short, masked description.
fn find_udid(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    // Modern form: 8 hex, '-', 16 hex.
    for i in 0..bytes.len() {
        if i + 25 <= bytes.len()
            && bytes[i + 8] == b'-'
            && bytes[i..i + 8].iter().all(u8::is_ascii_hexdigit)
            && bytes[i + 9..i + 25].iter().all(u8::is_ascii_hexdigit)
        {
            let prefix = &s[i..i + 8];
            if !prefix.eq_ignore_ascii_case("00000000") {
                return Some(format!("UDID-shaped {}…", &s[i..i + 9]));
            }
        }
    }
    // Legacy form: 40 hex characters in a row (not a longer hex run, which is likely a hash or base64-free blob).
    let mut run = 0usize;
    let mut start = 0usize;
    for (i, c) in bytes.iter().enumerate() {
        if c.is_ascii_hexdigit() {
            if run == 0 {
                start = i;
            }
            run += 1;
        } else {
            if run == 40 {
                return Some(format!("40-hex UDID-shaped {}…", &s[start..start + 8]));
            }
            run = 0;
        }
    }
    if run == 40 {
        return Some(format!("40-hex UDID-shaped {}…", &s[start..start + 8]));
    }
    None
}

/// A MAC address `XX:XX:XX:XX:XX:XX`.
fn find_mac(s: &str) -> Option<String> {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if i + 17 <= b.len() {
            let w = &b[i..i + 17];
            let ok =
                w.iter().enumerate().all(|(k, c)| if (k + 1) % 3 == 0 { *c == b':' } else { c.is_ascii_hexdigit() });
            if ok {
                // Ignore an all-zero MAC (a plausible placeholder).
                if &s[i..i + 17] != "00:00:00:00:00:00" {
                    return Some(format!("MAC {}…", &s[i..i + 8]));
                }
            }
        }
    }
    None
}

/// A plausible email address `local@domain.tld`.
fn find_email(s: &str) -> Option<String> {
    let at = s.find('@')?;
    let (local, rest) = (&s[..at], &s[at + 1..]);
    let local_ok = local.bytes().rev().take_while(|c| is_atom(*c)).count() > 0;
    let dot = rest.find('.')?;
    let domain = &rest[..dot];
    let tld: String = rest[dot + 1..].bytes().take_while(|c| c.is_ascii_alphabetic()).map(char::from).collect();
    let domain_ok = !domain.is_empty() && domain.bytes().all(|c| is_atom(c) || c == b'-');
    if local_ok && domain_ok && tld.len() >= 2 {
        // example.com / example.org and friends are the documented pseudonyms; allow them.
        if domain.eq_ignore_ascii_case("example") {
            return None;
        }
        return Some(format!("email …@{domain}.{tld}"));
    }
    None
}

fn is_atom(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'+' | b'%')
}

/// Applies the `default-v1` redaction to a facade result or arguments before it is recorded: replaces the values
/// of known-sensitive keys with stable synthetic pseudonyms. Keys outside the set are left alone (the lint is the
/// safety net for anything missed).
pub fn redact_value(v: &mut Value) {
    if let Value::Object(map) = v {
        for (k, val) in map.iter_mut() {
            match k.as_str() {
                "UniqueDeviceID"
                | "SerialNumber"
                | "UniqueChipID"
                | "ECID"
                | "InternationalMobileEquipmentIdentity"
                | "HostID"
                | "SystemBUID"
                | "PhoneNumber" => *val = Value::String(format!("REDACTED-{k}")),
                "WiFiAddress" | "BluetoothAddress" | "EthernetAddress" => {
                    *val = Value::String("00:00:00:00:00:00".into());
                }
                "DeviceName" => *val = Value::String("Russet Test iPhone".into()),
                _ => redact_value(val),
            }
        }
    } else if let Value::Array(a) = v {
        a.iter_mut().for_each(redact_value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strings_pass() {
        for s in ["27.0", "iPhone13,2", "00000000-0000000000000001", "dev.russet.test.probe", "me@example.com"] {
            let mut out = Vec::new();
            scan_text(1, s, &mut out);
            assert!(out.is_empty(), "{s} → {out:?}");
        }
    }

    #[test]
    fn real_identifiers_are_caught() {
        let cases = [
            ("00008101-0000000000000000", "udid"),
            ("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0", "udid"),
            ("ac:de:48:00:11:22", "mac"),
            ("victim@gmail.com", "email"),
            ("-----BEGIN EC PRIVATE KEY-----", "private-key"),
        ];
        for (s, kind) in cases {
            let mut out = Vec::new();
            scan_text(1, s, &mut out);
            assert!(out.iter().any(|f| f.kind == kind), "{s} should trip {kind}, got {out:?}");
        }
    }

    #[test]
    fn macho_magic_is_an_apple_file() {
        assert_eq!(apple_file_magic(&[0xcf, 0xfa, 0xed, 0xfe, 0, 0]), Some("a Mach-O binary"));
        assert_eq!(apple_file_magic(b"not a binary"), None);
    }

    #[test]
    fn redaction_replaces_sensitive_keys() {
        let mut v = serde_json::json!({ "UniqueDeviceID": "00008101-0000000000000000", "ProductType": "iPhone13,2" });
        redact_value(&mut v);
        assert_eq!(v["UniqueDeviceID"], serde_json::json!("REDACTED-UniqueDeviceID"));
        assert_eq!(v["ProductType"], serde_json::json!("iPhone13,2"));
        assert!(lint_value_clean(&v));
    }

    fn lint_value_clean(v: &Value) -> bool {
        let mut out = Vec::new();
        lint_value(1, v, &mut out);
        out.is_empty()
    }
}
