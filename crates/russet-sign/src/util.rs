// SPDX-License-Identifier: GPL-3.0-or-later
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
}
