// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Small helpers shared by the modules: error construction, glob matching, plist I/O and a sorted file walk.

use std::path::{Path, PathBuf};

use russet_core::error::{Error, codes};

/// `core.io` with the path and the cause.
pub(crate) fn io_error(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::new(codes::core::IO, format!("{what} {}: {e}", path.display())).caused_by(e)
}

/// `sign.bundleInvalid` with a hint.
pub(crate) fn bundle_invalid(path: &Path, why: impl std::fmt::Display) -> Error {
    Error::new(codes::sign::BUNDLE_INVALID, format!("{} is not a signable bundle: {why}", path.display()))
        .with_hint("Point at a built .app (or .ipa) whose Info.plist has CFBundleIdentifier and CFBundleExecutable.")
}

/// `sign.codesignFailed` wrapping a cause.
pub(crate) fn codesign_failed(
    what: impl std::fmt::Display,
    e: impl std::error::Error + Send + Sync + 'static,
) -> Error {
    Error::new(codes::sign::CODESIGN_FAILED, format!("{what}: {e}"))
        .with_hint("Run `russet sign verify` on the output and `russet doctor`; report the error if it persists.")
        .caused_by(e)
}

/// Apple's entitlement and application-identifier wildcard: `*` matches any run of characters (including
/// none); everything else matches literally.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// Reads a plist dictionary (XML or binary).
pub(crate) fn read_plist_dict(path: &Path) -> Result<plist::Dictionary, Error> {
    let v = plist::Value::from_file(path).map_err(|e| {
        Error::new(codes::core::INVALID_ARGUMENT, format!("cannot read the property list {}: {e}", path.display()))
            .with_hint("The file must be an XML or binary property list.")
            .caused_by(e)
    })?;
    v.into_dictionary().ok_or_else(|| {
        Error::new(codes::core::INVALID_ARGUMENT, format!("{} is not a dictionary property list", path.display()))
    })
}

/// Parses a plist dictionary from bytes (XML or binary).
pub(crate) fn plist_dict_from_bytes(data: &[u8], what: &str) -> Result<plist::Dictionary, Error> {
    let v = plist::Value::from_reader(std::io::Cursor::new(data)).map_err(|e| {
        Error::new(codes::core::INVALID_ARGUMENT, format!("cannot parse {what} as a property list: {e}")).caused_by(e)
    })?;
    v.into_dictionary()
        .ok_or_else(|| Error::new(codes::core::INVALID_ARGUMENT, format!("{what} is not a dictionary property list")))
}

/// Serializes a dictionary as an XML plist string.
pub(crate) fn plist_xml(dict: &plist::Dictionary) -> Result<String, Error> {
    let mut buf = Vec::new();
    plist::Value::Dictionary(dict.clone())
        .to_writer_xml(&mut buf)
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot serialize entitlements: {e}")).caused_by(e))?;
    String::from_utf8(buf).map_err(|e| Error::new(codes::core::INTERNAL, format!("entitlements XML: {e}")))
}

/// Every regular file and symlink under `root`, sorted, as paths relative to `root`. Symlinks are not followed.
pub(crate) fn walk_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let dir = root.join(&rel);
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|e| io_error("cannot read the directory", &dir, e))?
            .collect::<Result<_, _>>()
            .map_err(|e| io_error("cannot read the directory", &dir, e))?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let child = rel.join(e.file_name());
            let ft = e.file_type().map_err(|err| io_error("cannot stat", &root.join(&child), err))?;
            if ft.is_dir() {
                stack.push(child);
            } else {
                out.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// A path relative to a bundle, with `/` separators on every platform (the CodeResources and wire form).
pub(crate) fn slash_path(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Uppercase hex without leading zeros (the form Apple's portal and xtool use for certificate serials).
pub fn normalize_serial(bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    let trimmed = hex.trim_start_matches('0');
    if trimmed.is_empty() { "0".into() } else { trimmed.into() }
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Masks a team identifier for display (`*******XYZ`): team IDs are not printed in logs or reports (AGENTS.md §8).
pub fn mask_team(team: &str) -> String {
    let n = team.chars().count();
    let keep = n.saturating_sub(3);
    team.chars().enumerate().map(|(i, c)| if i < keep { '*' } else { c }).collect()
}

/// Whether `s` has the shape of a team ID or App ID prefix: exactly ten ASCII uppercase letters or digits.
pub fn is_team_like(s: &str) -> bool {
    s.len() == 10 && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// Masks the App ID prefix of an entitlement value or App ID: `ABCDE12345.com.example.app` becomes
/// `*******345.com.example.app`, `ABCDE12345.*` becomes `*******345.*`, and a bare team ID is masked whole.
/// Anything else is returned unchanged.
pub fn redact_app_id(value: &str) -> String {
    let (head, rest) = match value.split_once('.') {
        Some((h, r)) => (h, Some(r)),
        None => (value, None),
    };
    if !is_team_like(head) {
        return value.to_owned();
    }
    match rest {
        Some(r) => format!("{}.{r}", mask_team(head)),
        None => mask_team(head),
    }
}

/// Replaces every occurrence of a known team ID in `text` by its mask (`*******XYZ`).
pub fn redact_teams(text: &str, teams: &[String]) -> String {
    let mut out = text.to_owned();
    for t in teams.iter().filter(|t| t.len() >= 4) {
        if out.contains(t.as_str()) {
            out = out.replace(t.as_str(), &mask_team(t));
        }
    }
    out
}

fn redact_json(v: serde_json::Value, teams: &[String]) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::String(s) => Value::String(redact_teams(&s, teams)),
        Value::Array(a) => Value::Array(a.into_iter().map(|x| redact_json(x, teams)).collect()),
        Value::Object(m) => {
            Value::Object(m.into_iter().map(|(k, x)| (redact_teams(&k, teams), redact_json(x, teams))).collect())
        }
        other => other,
    }
}

/// An error cause that crossed a redaction: its text, then the next cause.
#[derive(Debug)]
struct RedactedCause {
    text: String,
    next: Option<Box<RedactedCause>>,
}

impl std::fmt::Display for RedactedCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for RedactedCause {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.next.as_deref().map(|n| n as &(dyn std::error::Error + 'static))
    }
}

/// `e` with every known team ID masked in its message, hint, details and cause chain (the daemon's wire boundary:
/// errors are assembled from paths, profiles and third-party messages that can name the team). An error that
/// names none of `teams` is returned unchanged.
pub fn redact_error(e: Error, teams: &[String]) -> Error {
    let data = e.to_data();
    let names_a_team = |s: &str| teams.iter().any(|t| t.len() >= 4 && s.contains(t.as_str()));
    let details_text = data.details.as_ref().map(serde_json::Value::to_string).unwrap_or_default();
    if !names_a_team(e.message())
        && !data.hint.as_deref().is_some_and(names_a_team)
        && !names_a_team(&details_text)
        && !data.chain.iter().any(|c| names_a_team(c))
    {
        return e;
    }
    let mut out = Error::new(e.code(), redact_teams(e.message(), teams)).retryable(data.retryable);
    if let Some(h) = data.hint {
        out = out.with_hint(redact_teams(&h, teams));
    }
    if let Some(d) = data.details {
        out = out.with_details(redact_json(d, teams));
    }
    let chain = data
        .chain
        .iter()
        .rev()
        .fold(None, |next, text| Some(Box::new(RedactedCause { text: redact_teams(text, teams), next })));
    if let Some(first) = chain {
        out = out.caused_by(*first);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("ABCDE12345.*", "ABCDE12345.com.example.app"));
        assert!(!glob_match("ABCDE12345.*", "ZZZZZ12345.com.example.app"));
        assert!(glob_match("com.example.*", "com.example."));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exactly"));
        assert!(glob_match("*.example.com", "app.example.com"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
    }

    #[test]
    fn serials_and_masks() {
        assert_eq!(normalize_serial(&[0x00, 0x0a, 0xbc]), "ABC");
        assert_eq!(normalize_serial(&[0x00]), "0");
        assert_eq!(mask_team("ABCDE12345"), "*******345");
        assert_eq!(mask_team("AB"), "AB");
    }

    #[test]
    fn app_ids_and_errors_are_redacted() {
        assert_eq!(redact_app_id("ABCDE12345.com.example.app"), "*******345.com.example.app");
        assert_eq!(redact_app_id("ABCDE12345.*"), "*******345.*");
        assert_eq!(redact_app_id("ABCDE12345"), "*******345");
        for keep in ["com.example.app", "group.com.example", "XTL-69A6DE7D.com.x", "abcde12345.x", "development"] {
            assert_eq!(redact_app_id(keep), keep);
        }
        let teams = vec!["ABCDE12345".to_owned()];
        assert_eq!(redact_teams("data/identities/ABCDE12345/1A.json", &teams), "data/identities/*******345/1A.json");

        let cause = std::io::Error::other("open /x/ABCDE12345/y failed");
        let e = Error::new(codes::core::IO, "cannot read /x/ABCDE12345/y")
            .with_hint("team ABCDE12345")
            .with_details(serde_json::json!({"path": "/x/ABCDE12345/y", "n": 1, "ABCDE12345": ["ABCDE12345.*"]}))
            .retryable(true)
            .caused_by(cause);
        let r = redact_error(e, &teams);
        let wire = format!("{} {}", r.message(), serde_json::to_string(&r.to_data()).unwrap_or_default());
        assert!(!wire.contains("ABCDE12345"), "{wire}");
        assert!(wire.contains("*******345"));
        assert_eq!(r.code(), codes::core::IO);
        assert!(r.to_data().retryable);
        assert_eq!(r.to_data().chain.len(), 1, "the cause chain is kept, redacted");
        let plain = Error::new(codes::core::IO, "nothing to hide");
        assert_eq!(redact_error(plain, &teams).message(), "nothing to hide");
    }
}
