// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Redaction for logs, the audit log and bug reports (ARCHITECTURE §12.2 "Logs", core §12; CORE-13).
//!
//! - [`is_sensitive_name`]: field names matching `token`, `password`, `secret`, `authorization`, `cookie`,
//!   `x-apple-i-md(-m)`, `anisette`, `private key`, `p8`, `p12` or `HostPrivateKey` (case-insensitive) have
//!   their values masked.
//! - [`redact_text`]: free text loses JWTs, PEM blocks, `Bearer`/`Basic` credentials and the values of
//!   `name=value`, `name: value` and `"name": "value"` pairs whose name is sensitive.
//! - [`redact_json`]: both rules over a JSON value (the audit log's `paramsRedacted`).
//! - [`IdentifierMask`]: for reports that leave the machine (`russet bugreport`), also the identifiers that
//!   ARCHITECTURE §12.1 and §11.6 treat as personal: UDIDs, serial numbers and ECIDs, Apple IDs (e-mail
//!   addresses), DSIDs, team IDs, App Store Connect key and issuer IDs, signing-identity names, device names
//!   and the home directory. Logs on the machine keep them.
//!
//! The canary tests feed known secrets through every log path and assert that none survives.

use std::borrow::Cow;
use std::collections::BTreeMap;

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

/// The kinds of identifier [`IdentifierMask`] replaces, and their placeholder names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum IdKind {
    Udid,
    Serial,
    Ecid,
    AppleId,
    Dsid,
    TeamId,
    KeyId,
    IssuerId,
    Identity,
    DeviceName,
}

impl IdKind {
    fn label(self) -> &'static str {
        match self {
            Self::Udid => "udid",
            Self::Serial => "serial",
            Self::Ecid => "ecid",
            Self::AppleId => "apple-id",
            Self::Dsid => "dsid",
            Self::TeamId => "team-id",
            Self::KeyId => "key-id",
            Self::IssuerId => "issuer-id",
            Self::Identity => "identity",
            Self::DeviceName => "device-name",
        }
    }

    /// The kind of value a key named `name` holds (compared like [`is_sensitive_name`]): the exact names below,
    /// and names that end in one of the suffixes (`allowed-teams`, `deviceUdid`, `ascKeyId`).
    fn of_key(name: &str) -> Option<Self> {
        let folded: String =
            name.chars().filter(|c| !matches!(c, '-' | '_' | '.' | ' ')).flat_map(char::to_lowercase).collect();
        let exact = match folded.as_str() {
            "uniquedeviceid" | "deviceid" | "devices" => Some(Self::Udid),
            "serial" => Some(Self::Serial),
            "uniquechipid" | "chipid" => Some(Self::Ecid),
            "email" | "emailaddress" | "accountname" | "username" => Some(Self::AppleId),
            "team" | "developmentteam" | "teamidentifier" | "seedid" => Some(Self::TeamId),
            "issuer" => Some(Self::IssuerId),
            _ => None,
        };
        const SUFFIXES: &[(&str, IdKind)] = &[
            ("udid", IdKind::Udid),
            ("udids", IdKind::Udid),
            ("serialnumber", IdKind::Serial),
            ("ecid", IdKind::Ecid),
            ("appleid", IdKind::AppleId),
            ("appleids", IdKind::AppleId),
            ("dsid", IdKind::Dsid),
            ("teamid", IdKind::TeamId),
            ("teamids", IdKind::TeamId),
            ("teams", IdKind::TeamId),
            ("keyid", IdKind::KeyId),
            ("issuerid", IdKind::IssuerId),
            ("devicename", IdKind::DeviceName),
        ];
        exact.or_else(|| SUFFIXES.iter().find(|(suffix, _)| folded.ends_with(suffix)).map(|(_, kind)| *kind))
    }
}

/// Signing-identity names: the person's name and the team ID in parentheses are both identifiers.
const IDENTITY_PREFIXES: &[&str] = &[
    "Apple Development: ",
    "Apple Distribution: ",
    "iPhone Developer: ",
    "iPhone Distribution: ",
    "Developer ID Application: ",
    "Mac Developer: ",
];

/// Prefixes of account and secret ids whose rest is an identifier (ARCHITECTURE §14.4, `SecretId`), and the
/// file name of an App Store Connect key.
const ID_PREFIXES: &[(&str, IdKind)] = &[
    ("asc:", IdKind::KeyId),
    ("appleid:", IdKind::Dsid),
    ("asc-key/", IdKind::KeyId),
    ("apple-id-session/", IdKind::Dsid),
    ("apple-id-password/", IdKind::Dsid),
    ("AuthKey_", IdKind::KeyId),
];

/// Replaces identifiers with stable placeholders for a report that leaves the machine (module docs).
///
/// Equal values get the same placeholder within one mask (`<udid-1>`, `<team-id-2>`), so a report still shows
/// which device or team a line is about without naming it. It masks values under identifier keys (`udid`,
/// `team-id`, `teamId`, `key-id`, `issuer-id`, `apple-id`, `dsid`, `deviceName`, …, in `key = value`, `key:
/// value` and JSON form, arrays included), modern and legacy UDIDs, e-mail addresses, account and secret ids
/// (`asc:<KEYID>`, `appleid:<DSID>`, `asc-key/<KEYID>`), signing-identity names (`Apple Development: Name
/// (TEAMID)`), `TEAMID.` application-identifier prefixes, the `name` of JSON objects that also have a `udid`,
/// and the home directory (as `~`). It does not mask secrets: run [`redact_text`] or [`redact_json`] too.
#[derive(Debug, Default)]
pub struct IdentifierMask {
    seen: BTreeMap<(IdKind, String), usize>,
    counts: BTreeMap<IdKind, usize>,
    home: Option<String>,
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_email_local(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn is_email_domain(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')
}

fn is_placeholder(v: &str) -> bool {
    v.starts_with('<') && v.ends_with('>') || v == MASK || v.starts_with("[redacted") || v.is_empty() || v == "~"
}

/// An unquoted value that names nothing: a boolean, `null`, or a small number (a count, a version).
fn is_plain_value(v: &str) -> bool {
    matches!(v, "true" | "false" | "null") || (v.len() < 6 && v.bytes().all(|b| b.is_ascii_digit()))
}

impl IdentifierMask {
    /// A mask that also replaces `home` (the user's home directory) with `~`.
    pub fn new(home: Option<&std::path::Path>) -> Self {
        let home = home.map(|h| h.display().to_string()).filter(|h| h.len() > 1);
        Self { home, ..Self::default() }
    }

    /// The placeholder of `value`.
    fn placeholder(&mut self, kind: IdKind, value: &str) -> String {
        let n = match self.seen.get(&(kind, value.to_owned())) {
            Some(n) => *n,
            None => {
                let c = self.counts.entry(kind).or_insert(0);
                *c += 1;
                self.seen.insert((kind, value.to_owned()), *c);
                *c
            }
        };
        format!("<{}-{n}>", kind.label())
    }

    /// Masks the identifiers in free text (one line or many).
    pub fn text(&mut self, s: &str) -> String {
        let mut out = match &self.home {
            Some(home) => s.replace(home.as_str(), "~"),
            None => s.to_owned(),
        };
        out = self.prefixed(&out);
        out = self.identities(&out);
        out = self.keyed(&out);
        out = self.udids(&out);
        out = self.emails(&out);
        self.team_prefixes(&out)
    }

    /// Masks the identifiers in a JSON value (module docs).
    pub fn json(&mut self, v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                let device_record = map.contains_key("udid");
                for (k, child) in map.iter_mut() {
                    let kind =
                        IdKind::of_key(k).or_else(|| (device_record && k == "name").then_some(IdKind::DeviceName));
                    match (kind, child) {
                        (Some(kind), serde_json::Value::String(s)) if !is_placeholder(s) => {
                            *s = self.placeholder(kind, s);
                        }
                        (Some(kind), serde_json::Value::Array(items)) => {
                            for item in items.iter_mut() {
                                match item {
                                    serde_json::Value::String(s) if !is_placeholder(s) => {
                                        *s = self.placeholder(kind, s);
                                    }
                                    other => self.json(other),
                                }
                            }
                        }
                        (_, child) => self.json(child),
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(|i| self.json(i)),
            serde_json::Value::String(s) => *s = self.text(s),
            _ => {}
        }
    }

    /// `key = value`, `key: value`, `"key": "value"` and `key = ["a", "b"]` under identifier keys.
    fn keyed(&mut self, s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let (mut copied, mut i) = (0, 0);
        while i < bytes.len() {
            if !(bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'-' | b'.')) {
                i += 1;
            }
            let Some(kind) = IdKind::of_key(&s[start..i]) else { continue };
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
            // An array: every quoted string up to the closing bracket.
            if j < bytes.len() && bytes[j] == b'[' {
                let Some(close) = s[j..].find(']').map(|c| j + c) else { continue };
                let mut k = j;
                while k < close {
                    if matches!(bytes[k], b'"' | b'\'') {
                        let q = bytes[k];
                        let Some(end) = s[k + 1..close].find(q as char).map(|e| k + 1 + e) else { break };
                        let value = &s[k + 1..end];
                        if !is_placeholder(value) {
                            out.push_str(&s[copied..k + 1]);
                            out.push_str(&self.placeholder(kind, value));
                            copied = end;
                        }
                        k = end + 1;
                    } else {
                        k += 1;
                    }
                }
                i = close;
                continue;
            }
            let quote = (j < bytes.len() && matches!(bytes[j], b'"' | b'\'')).then(|| bytes[j]);
            if quote.is_some() {
                j += 1;
            }
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
            let value = &s[value_start..j];
            if !is_placeholder(value) && !(quote.is_none() && is_plain_value(value)) {
                out.push_str(&s[copied..value_start]);
                out.push_str(&self.placeholder(kind, value));
                copied = j;
            }
            i = j;
        }
        out.push_str(&s[copied..]);
        out
    }

    /// `Apple Development: Name (TEAMID)` and the other signing-identity forms.
    fn identities(&mut self, s: &str) -> String {
        let mut out = s.to_owned();
        for prefix in IDENTITY_PREFIXES {
            let mut from = 0;
            while let Some(pos) = out[from..].find(prefix).map(|p| p + from) {
                let start = pos + prefix.len();
                let Some(open) = out[start..].find(" (").map(|o| start + o) else { break };
                let Some(close) = out[open..].find(')').map(|c| open + c) else { break };
                let name = out[start..open].to_owned();
                let team = out[open + 2..close].to_owned();
                if name.contains('\n') || is_placeholder(&name) {
                    from = close;
                    continue;
                }
                let replacement = format!(
                    "{} ({})",
                    self.placeholder(IdKind::Identity, &name),
                    self.placeholder(IdKind::TeamId, &team)
                );
                out.replace_range(start..=close, &replacement);
                from = start + replacement.len();
            }
        }
        out
    }

    /// `asc:<KEYID>`, `appleid:<DSID>`, `asc-key/<KEYID>`, ….
    fn prefixed(&mut self, s: &str) -> String {
        let mut out = s.to_owned();
        for (prefix, kind) in ID_PREFIXES {
            let mut from = 0;
            while let Some(pos) = out[from..].find(prefix).map(|p| p + from) {
                let start = pos + prefix.len();
                let end = out[start..]
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                    .map_or(out.len(), |e| start + e);
                let value = out[start..end].to_owned();
                let boundary = pos == 0 || !is_word(out.as_bytes()[pos - 1]);
                if !boundary || value.is_empty() || is_placeholder(&value) {
                    from = start;
                    continue;
                }
                let replacement = self.placeholder(*kind, &value);
                out.replace_range(start..end, &replacement);
                from = start + replacement.len();
            }
        }
        out
    }

    /// Modern (`00008101-001A2B3C4D5E001E`) and legacy (40 hex digits) UDIDs.
    fn udids(&mut self, s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let (mut copied, mut i) = (0, 0);
        while i < bytes.len() {
            if !is_hex(bytes[i]) || (i > 0 && (is_word(bytes[i - 1]) || bytes[i - 1] == b'-')) {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < bytes.len() && is_hex(bytes[j]) {
                j += 1;
            }
            let run = j - i;
            let end = if run == 8 && j < bytes.len() && bytes[j] == b'-' {
                let mut k = j + 1;
                while k < bytes.len() && is_hex(bytes[k]) {
                    k += 1;
                }
                (k - j - 1 == 16).then_some(k)
            } else {
                (run == 40).then_some(j)
            };
            match end {
                Some(end) if end == bytes.len() || !(is_word(bytes[end]) || bytes[end] == b'-') => {
                    out.push_str(&s[copied..i]);
                    out.push_str(&self.placeholder(IdKind::Udid, &s[i..end].to_ascii_uppercase()));
                    copied = end;
                    i = end;
                }
                _ => i = j.max(i + 1),
            }
        }
        out.push_str(&s[copied..]);
        out
    }

    /// E-mail addresses (Apple IDs).
    fn emails(&mut self, s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let mut copied = 0;
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'@' {
                i += 1;
                continue;
            }
            let mut start = i;
            while start > copied && is_email_local(bytes[start - 1]) {
                start -= 1;
            }
            let mut end = i + 1;
            while end < bytes.len() && is_email_domain(bytes[end]) {
                end += 1;
            }
            while end > i + 1 && bytes[end - 1] == b'.' {
                end -= 1;
            }
            let domain = &s[i + 1..end];
            let tld_ok = domain
                .rsplit_once('.')
                .is_some_and(|(_, tld)| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()));
            if start < i && tld_ok {
                out.push_str(&s[copied..start]);
                out.push_str(&self.placeholder(IdKind::AppleId, &s[start..end].to_ascii_lowercase()));
                copied = end;
                i = end;
            } else {
                i += 1;
            }
        }
        out.push_str(&s[copied..]);
        out
    }

    /// `TEAMID.com.example.app`: the team prefix of an application identifier.
    fn team_prefixes(&mut self, s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let (mut copied, mut i) = (0, 0);
        while i + 11 < bytes.len() {
            let boundary = i == 0 || !(is_word(bytes[i - 1]) || matches!(bytes[i - 1], b'.' | b'-' | b'<'));
            let candidate = &bytes[i..i + 10];
            if boundary
                && candidate.iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                && candidate.iter().any(u8::is_ascii_digit)
                && candidate.iter().any(u8::is_ascii_uppercase)
                && bytes[i + 10] == b'.'
                && bytes[i + 11].is_ascii_lowercase()
            {
                out.push_str(&s[copied..i]);
                out.push_str(&self.placeholder(IdKind::TeamId, &s[i..i + 10]));
                copied = i + 10;
                i += 10;
                continue;
            }
            i += 1;
        }
        out.push_str(&s[copied..]);
        out
    }
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
    fn identifiers_get_stable_placeholders() {
        let mut m = IdentifierMask::new(Some(std::path::Path::new("/home/someone")));
        let text = "attached 00008101-001A2B3C4D5E001E (udid=00008101-001A2B3C4D5E001E) and \
                    0123456789abcdef0123456789abcdef01234567; user someone.person@example.com; \
                    team-id = \"ZX9Y8W7V6U\"; key-id: QW3ERT5YU7; issuer-id = \"c4a7a7e1-0000-4000-8000-000000000001\"; \
                    account asc:QW3ERT5YU7 and appleid:12345678901, secret:asc-key/QW3ERT5YU7; \
                    signed by Apple Development: Some Person (ZX9Y8W7V6U) as ZX9Y8W7V6U.com.example.app \
                    in /home/someone/work; deviceName=\"Some's iPhone\" teams = [\"AB12CD34EF\", 'ZX9Y8W7V6U']";
        let out = m.text(text);
        for gone in [
            "00008101-001A2B3C4D5E001E",
            "0123456789abcdef0123456789abcdef01234567",
            "someone.person@example.com",
            "ZX9Y8W7V6U",
            "QW3ERT5YU7",
            "c4a7a7e1-0000-4000-8000-000000000001",
            "12345678901",
            "Some Person",
            "Some's iPhone",
            "AB12CD34EF",
            "/home/someone",
        ] {
            assert!(!out.contains(gone), "{gone} survived in {out}");
        }
        assert_eq!(out.matches("<udid-1>").count(), 2, "one device, one placeholder: {out}");
        assert!(out.contains("<team-id-1>") && out.contains("~/work") && out.contains("com.example.app"), "{out}");
        assert_eq!(m.text("again 00008101-001a2b3c4d5e001e"), "again <udid-1>", "case-insensitive and stable");
        // Placeholders, digests, versions and ordinary words stay.
        let keep =
            "sha256 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08 v0.1.0+g1a2b3c4 <team-id-1>";
        assert_eq!(m.text(keep), keep);
        let mut v = json!({"leases": [{"resource": "00008101-001A2B3C4D5E001E", "udid": "00008101-001A2B3C4D5E001E",
                                       "name": "Some's iPhone"}],
                           "teamId": "ZX9Y8W7V6U", "connections": [{"name": "russet", "kind": "cli"}]});
        m.json(&mut v);
        assert_eq!(v["leases"][0]["udid"], "<udid-1>");
        assert_eq!(v["leases"][0]["resource"], "<udid-1>");
        assert_eq!(v["leases"][0]["name"], "<device-name-1>");
        assert_eq!(v["teamId"], "<team-id-1>");
        assert_eq!(v["connections"][0]["name"], "russet");
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
