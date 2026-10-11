// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Redaction helpers (ARCHITECTURE §12.2 "Logs"; core §12) [D 13] for the logging layer (W2, CORE-13), the bug
//! report and anything else that might print data: a field-name rule and a value mask for JWTs, PEM blocks and
//! `Bearer` tokens. Nothing here depends on a regex engine.

use std::borrow::Cow;

/// What replaces a redacted value.
pub const MASK: &str = "***";

/// Field-name fragments whose values are always redacted (case-insensitive; `-`, `_`, `.` and spaces are
/// ignored): `token`, `password`, `secret`, `authorization`, `cookie`, `x-apple-i-md*`, `anisette`, `private key`,
/// `p8`, `p12`, `HostPrivateKey`.
const SENSITIVE: &[&str] =
    &["token", "password", "secret", "authorization", "cookie", "xappleimd", "anisette", "privatekey", "passphrase"];

fn normalize(name: &str) -> String {
    name.chars().filter(|c| !matches!(c, '-' | '_' | '.' | ' ')).flat_map(char::to_lowercase).collect()
}

/// Whether a field (or header, or JSON key) named `name` carries a secret.
pub fn is_sensitive_field(name: &str) -> bool {
    let n = normalize(name);
    if SENSITIVE.iter().any(|s| n.contains(s)) {
        return true;
    }
    // `p8`/`p12` only as whole words (`ascP8`, `p12Password`), not inside identifiers like `udp8080`.
    let lower = name.to_ascii_lowercase();
    lower.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| matches!(w, "p8" | "p12"))
        || name.ends_with("P8")
        || name.ends_with("P12")
        || name.contains("P12")
}

fn is_b64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Length of a JWT (`eyJ….eyJ….sig`) starting at `i`, or 0.
fn jwt_len(s: &[u8], i: usize) -> usize {
    if !s[i..].starts_with(b"eyJ") || (i > 0 && is_b64url(s[i - 1])) {
        return 0;
    }
    let mut j = i;
    let mut dots = 0;
    while j < s.len() && (is_b64url(s[j]) || (s[j] == b'.' && dots < 2)) {
        if s[j] == b'.' {
            dots += 1;
        }
        j += 1;
    }
    if dots == 2 && j - i > 20 { j - i } else { 0 }
}

/// Length of a PEM block (`-----BEGIN …-----` to the end of its `-----END …-----` line) starting at `i`, or 0.
fn pem_len(s: &str, i: usize) -> usize {
    let rest = &s[i..];
    if !rest.starts_with("-----BEGIN ") {
        return 0;
    }
    let Some(end) = rest.find("-----END ") else { return rest.len() };
    let after = &rest[end + "-----END ".len()..];
    match after.find("-----") {
        Some(close) => end + "-----END ".len() + close + "-----".len(),
        None => rest.len(),
    }
}

/// Length of `Bearer <token>` starting at `i` (the token part only is masked), or 0.
fn bearer_token(s: &str, i: usize) -> Option<(usize, usize)> {
    let rest = &s[i..];
    let prefix_len = "bearer ".len();
    if rest.len() <= prefix_len || !rest[..prefix_len].eq_ignore_ascii_case("bearer ") {
        return None;
    }
    if i > 0 && s.as_bytes()[i - 1].is_ascii_alphanumeric() {
        return None;
    }
    let token = &rest[prefix_len..];
    let len = token.bytes().take_while(|b| !b.is_ascii_whitespace() && *b != b'"' && *b != b'\'' && *b != b',').count();
    (len > 0).then_some((prefix_len, len))
}

/// Masks JWTs, PEM blocks and `Bearer` tokens inside free text.
pub fn mask(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let (start, len) = if let Some((skip, len)) = bearer_token(text, i) {
            (i + skip, len)
        } else {
            let pem = pem_len(text, i);
            if pem > 0 { (i, pem) } else { (i, jwt_len(bytes, i)) }
        };
        if len == 0 {
            i += 1;
            continue;
        }
        let o = out.get_or_insert_with(|| String::with_capacity(text.len()));
        o.push_str(&text[copied..start]);
        o.push_str(MASK);
        copied = start + len;
        i = copied;
    }
    match out {
        None => Cow::Borrowed(text),
        Some(mut o) => {
            o.push_str(&text[copied..]);
            Cow::Owned(o)
        }
    }
}

/// Redacts a JSON value in place: values of sensitive keys become [`MASK`], and strings are [`mask`]ed.
pub fn redact_json(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if is_sensitive_field(k) && !val.is_null() {
                    *val = serde_json::Value::String(MASK.into());
                } else {
                    redact_json(val);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact_json),
        serde_json::Value::String(s) => {
            if let Cow::Owned(m) = mask(s) {
                *s = m;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_field_names() {
        for name in [
            "token",
            "accessToken",
            "Authorization",
            "x-apple-i-md-m",
            "X-Apple-I-MD",
            "cookie",
            "HostPrivateKey",
            "private_key",
            "privateKey",
            "ascP8",
            "p12",
            "RUSSET_SIGNING_P12_PASSWORD",
            "anisette",
            "passphrase",
            "client_secret",
        ] {
            assert!(is_sensitive_field(name), "{name}");
        }
        for name in ["udid", "bundleId", "path", "udp8080", "deviceName", "label"] {
            assert!(!is_sensitive_field(name), "{name}");
        }
    }

    #[test]
    fn values_are_masked() {
        let jwt = "eyJhbGciOiJFUzI1NiIsImtpZCI6IjJYOVIifQ.eyJpc3MiOiJ4In0.c2lnbmF0dXJlLXZhbHVl";
        let line = format!("calling ASC with {jwt} now");
        assert_eq!(mask(&line), "calling ASC with *** now");
        assert_eq!(mask("Authorization: Bearer abc.def-123, next"), "Authorization: Bearer ***, next");
        let pem = "key:\n-----BEGIN PRIVATE KEY-----\nMIGTAgEA\n-----END PRIVATE KEY-----\ndone";
        assert_eq!(mask(pem), "key:\n***\ndone");
        assert!(matches!(mask("nothing secret here"), Cow::Borrowed(_)));
        assert_eq!(mask("unterminated -----BEGIN RSA PRIVATE KEY-----\nAAAA"), "unterminated ***");
        let mut v = serde_json::json!({"password": "hunter2", "nested": [{"note": format!("x {jwt}")}], "udid": "u"});
        redact_json(&mut v);
        assert_eq!(v, serde_json::json!({"password": "***", "nested": [{"note": "x ***"}], "udid": "u"}));
    }
}
