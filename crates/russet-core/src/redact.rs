// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Redaction for logs, the audit log and bug reports (ARCHITECTURE §12.2 "Logs", core §12; CORE-13).
//!
//! - [`is_sensitive_name`]: field names matching `token`, `password`, `secret`, `authorization`, `cookie`,
//!   `x-apple-i-md(-m)`, `anisette`, `private key`, `p8`, `p12` or `HostPrivateKey` (case-insensitive) have
//!   their values masked.
//! - [`redact_text`]: free text loses JWTs, PEM blocks, `Bearer`/`Basic` credentials and the values of
//!   `name=value`, `name: value` and `"name": "value"` pairs whose name is sensitive.
//! - [`redact_json`]: both rules over a JSON value (the audit log's `paramsRedacted`).
//!
//! The canary tests feed known secrets through every log path and assert that none survives.

use std::borrow::Cow;

/// What replaces a masked value.
pub const MASK: &str = "***";

/// Name fragments whose values are secrets (compared case-insensitively, ignoring `-`, `_`, `.` and spaces).
const SENSITIVE: &[&str] = &[
    "token",
    "password",
    "passwd",
    "passphrase",
    "secret",
    "authorization",
    "cookie",
    "xappleimd",
    "anisette",
    "privatekey",
    "apikey",
    "p8",
    "p12",
    "credential",
];

/// Whether a field or key named `name` holds a secret.
pub fn is_sensitive_name(name: &str) -> bool {
    let folded: String =
        name.chars().filter(|c| !matches!(c, '-' | '_' | '.' | ' ')).flat_map(char::to_lowercase).collect();
    SENSITIVE.iter().any(|s| folded.contains(s))
}

/// Masks secrets in free text. Returns the input unchanged (borrowed) when nothing matched.
pub fn redact_text(s: &str) -> Cow<'_, str> {
    if !might_contain_secret(s) {
        return Cow::Borrowed(s);
    }
    let out = mask_pem(s);
    let out = mask_jwts(&out);
    let out = mask_auth_schemes(&out);
    let out = mask_assignments(&out);
    if out == s { Cow::Borrowed(s) } else { Cow::Owned(out) }
}

/// Masks secrets inside a JSON value: values under sensitive keys become [`MASK`], strings go through
/// [`redact_text`].
pub fn redact_json(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, child) in map.iter_mut() {
                if is_sensitive_name(k) && !child.is_null() && !matches!(child, serde_json::Value::Bool(_)) {
                    *child = serde_json::Value::String(MASK.to_owned());
                } else {
                    redact_json(child);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact_json),
        serde_json::Value::String(s) => {
            if let Cow::Owned(red) = redact_text(s) {
                *s = red;
            }
        }
        _ => {}
    }
}

fn might_contain_secret(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("eyj")
        || lower.contains("-----begin")
        || lower.contains("bearer")
        || lower.contains("basic ")
        || SENSITIVE.iter().any(|k| lower.contains(k))
        || lower.contains("private")
        || lower.contains("x-apple")
}

fn mask_pem(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("-----BEGIN ") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        // The END line closes the block; without one, everything after BEGIN is masked.
        let end = after.find("-----END ").and_then(|e| after[e + 9..].find("-----").map(|close| e + 9 + close + 5));
        out.push_str("[redacted PEM block]");
        match end {
            Some(e) => rest = &after[e..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_b64url(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'='
}

fn mask_jwts(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut copied = 0;
    while i + 3 <= bytes.len() {
        if &bytes[i..i + 3] == b"eyJ" && (i == 0 || !is_b64url(bytes[i - 1])) {
            // header . payload [. signature]
            let mut j = i;
            let mut dots = 0;
            while j < bytes.len() && (is_b64url(bytes[j]) || bytes[j] == b'.') {
                if bytes[j] == b'.' {
                    dots += 1;
                }
                j += 1;
            }
            if dots >= 1 && j - i >= 10 {
                out.push_str(&s[copied..i]);
                out.push_str("[redacted JWT]");
                copied = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[copied..]);
    out
}

fn mask_auth_schemes(s: &str) -> String {
    let mut out = s.to_owned();
    for scheme in ["bearer ", "basic "] {
        let mut search_from = 0;
        loop {
            let lower = out.to_ascii_lowercase();
            let Some(pos) = lower[search_from..].find(scheme).map(|p| p + search_from) else { break };
            let value_start = pos + scheme.len();
            let value_end = out[value_start..]
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '}' | ']' | ')'))
                .map_or(out.len(), |e| value_start + e);
            if value_end > value_start && &out[value_start..value_end] != MASK {
                out.replace_range(value_start..value_end, MASK);
            }
            search_from = value_start + MASK.len().min(out.len() - value_start);
        }
    }
    out
}

/// Masks the values of `name=value`, `name: value` and `"name": "value"` where `name` is sensitive.
fn mask_assignments(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        // A name: a run of identifier characters.
        if !(bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-') {
            i += 1;
            continue;
        }
        let name_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'-' | b'.')) {
            i += 1;
        }
        let name = &s[name_start..i];
        if !is_sensitive_name(name) {
            continue;
        }
        // Optional closing quote of a JSON key, spaces, then `=` or `:`.
        let mut j = i;
        if j < bytes.len() && matches!(bytes[j], b'"' | b'\'') {
            j += 1;
        }
        while j < bytes.len() && bytes[j] == b' ' {
            j += 1;
        }
        if j >= bytes.len() || !matches!(bytes[j], b'=' | b':') {
            continue;
        }
        j += 1;
        while j < bytes.len() && bytes[j] == b' ' {
            j += 1;
        }
        let quote = if j < bytes.len() && matches!(bytes[j], b'"' | b'\'') {
            j += 1;
            Some(bytes[j - 1])
        } else {
            None
        };
        let value_start = j;
        while j < bytes.len() {
            let c = bytes[j];
            let stop = match quote {
                Some(q) => c == q,
                None => c.is_ascii_whitespace() || matches!(c, b',' | b';' | b'&' | b'}' | b']' | b')'),
            };
            if stop {
                break;
            }
            j += 1;
        }
        if j > value_start && &s[value_start..j] != MASK && !s[value_start..j].starts_with("[redacted") {
            out.push_str(&s[copied..value_start]);
            out.push_str(MASK);
            copied = j;
        }
        i = j;
    }
    out.push_str(&s[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const CANARY: &str = "RuSsEtCaNaRy7f3a9";

    #[test]
    fn sensitive_names_follow_the_list() {
        for n in [
            "token",
            "authToken",
            "X-Apple-I-MD-M",
            "password",
            "client_secret",
            "Authorization",
            "cookie",
            "anisetteState",
            "privateKey",
            "private key",
            "HostPrivateKey",
            "p8",
            "keyP12",
            "apiKey",
        ] {
            assert!(is_sensitive_name(n), "{n}");
        }
        for n in ["name", "path", "udid", "message", "bundleId", "kind"] {
            assert!(!is_sensitive_name(n), "{n}");
        }
    }

    #[test]
    fn text_secrets_are_masked() {
        let jwt = format!("eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJ{CANARY}.c2lnbmF0dXJl");
        let pem = format!("-----BEGIN PRIVATE KEY-----\nMIG{CANARY}\n-----END PRIVATE KEY-----");
        let cases = [
            format!("calling ASC with {jwt} now"),
            format!("Authorization: Bearer {CANARY}"),
            format!("key file:\n{pem}\nend"),
            format!("password={CANARY}&user=me"),
            format!("{{\"token\": \"{CANARY}\", \"udid\": \"x\"}}"),
            format!("token: {CANARY}"),
            format!("basic {CANARY}"),
        ];
        for case in cases {
            let red = redact_text(&case);
            assert!(!red.contains(CANARY), "canary survived in {red:?}");
        }
        assert_eq!(redact_text("user=me path=/tmp/x"), "user=me path=/tmp/x");
        assert!(matches!(redact_text("nothing to see"), Cow::Borrowed(_)));
        assert_eq!(redact_text("password=hunter2 user=me"), "password=*** user=me");
    }

    #[test]
    fn json_values_are_masked() {
        let mut v = json!({"name": "me", "token": CANARY, "nested": {"apiKey": [CANARY]},
                           "note": format!("Bearer {CANARY}"), "interactive": true, "secretRef": null});
        redact_json(&mut v);
        let text = v.to_string();
        assert!(!text.contains(CANARY), "{text}");
        assert_eq!(v["name"], "me");
        assert_eq!(v["token"], MASK);
        assert_eq!(v["secretRef"], serde_json::Value::Null);
    }
}
