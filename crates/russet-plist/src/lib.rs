// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! XML and binary plist I/O and Xcode build-setting macro expansion (build B01). Layer F: permissive, and no
//! `russet-*` dependency except other F crates (ARCHITECTURE §5.2).
//!
//! - Reading goes through the [`plist`] crate, which keeps dictionary order (and reads OpenStep too).
//! - Writing is Russet's own, so that a value read from an Apple-written file re-serializes byte for byte: the
//!   XML writer follows the layout Apple's tools produce (tab indents, `<dict/>` for empty collections, only `&`,
//!   `<` and `>` escaped), and the binary writer flattens and uniques objects the way CoreFoundation does
//!   (containers depth-first with dictionary keys before values; strings, numbers, booleans, dates and data
//!   uniqued; the smallest reference and offset sizes).
//! - [`macros`] expands `$(VAR)`, `${VAR}`, `$VAR`, `$(VAR:op…)`, nested `$($(VAR))` and `$(inherited)`.
//!
//! The OpenStep writer is W16's (B01 split, PLAN §15).

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::io::Cursor;
use std::path::Path;

pub use plist;
pub use plist::{Dictionary, Value};

mod binary;
pub mod macros;
mod xml;

/// An on-disk plist encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// `<?xml …?><plist version="1.0">…` (Apple's XML property list).
    Xml,
    /// `bplist00…`.
    Binary,
    /// The old ASCII (OpenStep) syntax: readable here, writable once W16 adds the writer.
    OpenStep,
}

/// Errors of this crate (layer F has no dependency on `russet-core`).
#[derive(Debug, thiserror::Error)]
pub enum PlistError {
    /// The bytes are not a property list in any supported encoding.
    #[error("invalid property list: {0}")]
    Parse(String),
    /// The value cannot be written in the requested encoding (for example a UID in XML, or a control
    /// character in an XML string).
    #[error("cannot write this property list: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// `Result` with [`PlistError`].
pub type Result<T, E = PlistError> = std::result::Result<T, E>;

/// Detects the encoding from the first bytes; `None` when it is none of the three.
pub fn detect_format(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"bplist00") {
        return Some(Format::Binary);
    }
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let start = text.iter().position(|b| !b.is_ascii_whitespace())?;
    let text = &text[start..];
    if text.starts_with(b"<?xml") || text.starts_with(b"<!DOCTYPE plist") || text.starts_with(b"<plist") {
        Some(Format::Xml)
    } else if matches!(text.first(), Some(b'{' | b'(' | b'"' | b'/')) || text[0].is_ascii_alphanumeric() {
        Some(Format::OpenStep)
    } else {
        None
    }
}

/// Parses a property list in any supported encoding and reports which one it was.
pub fn from_bytes(bytes: &[u8]) -> Result<(Value, Format)> {
    let format = detect_format(bytes).ok_or_else(|| PlistError::Parse("not a property list".into()))?;
    let value = match format {
        Format::Binary | Format::Xml => Value::from_reader(Cursor::new(bytes)),
        Format::OpenStep => Value::from_reader_ascii(Cursor::new(bytes)),
    }
    .map_err(|e| PlistError::Parse(e.to_string()))?;
    Ok((value, format))
}

/// Reads a property list file in any supported encoding.
pub fn from_path(path: impl AsRef<Path>) -> Result<(Value, Format)> {
    let bytes = std::fs::read(path.as_ref())?;
    from_bytes(&bytes)
}

/// Reads a property list file whose root must be a dictionary.
pub fn dictionary_from_path(path: impl AsRef<Path>) -> Result<Dictionary> {
    match from_path(path)?.0 {
        Value::Dictionary(d) => Ok(d),
        _ => Err(PlistError::Parse("the root object is not a dictionary".into())),
    }
}

/// Serializes `value` in Apple's XML layout.
pub fn to_xml(value: &Value) -> Result<Vec<u8>> {
    xml::write(value)
}

/// Serializes `value` as a binary plist, flattened and uniqued as CoreFoundation does.
pub fn to_binary(value: &Value) -> Result<Vec<u8>> {
    binary::write(value)
}

/// Serializes `value` in `format`.
pub fn to_bytes(value: &Value, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Xml => to_xml(value),
        Format::Binary => to_binary(value),
        Format::OpenStep => Err(PlistError::Unsupported("the OpenStep writer is not implemented yet (W16)".into())),
    }
}

/// Writes `value` to `path` in `format` (not atomically; callers that need atomicity write to a temporary file
/// and rename it).
pub fn to_path(path: impl AsRef<Path>, value: &Value, format: Format) -> Result<()> {
    std::fs::write(path, to_bytes(value, format)?)?;
    Ok(())
}

/// Encodes `data` as base64 with padding (RFC 4648), as plist `<data>` and JSON forms use.
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let symbols = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, s) in symbols.iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[*s as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_detected() {
        assert_eq!(detect_format(b"bplist00\xd0\x08"), Some(Format::Binary));
        assert_eq!(detect_format(b"<?xml version=\"1.0\"?><plist/>"), Some(Format::Xml));
        assert_eq!(detect_format(b"\xEF\xBB\xBF  <plist version=\"1.0\"/>"), Some(Format::Xml));
        assert_eq!(detect_format(b"{ a = b; }"), Some(Format::OpenStep));
        assert_eq!(detect_format(b"// !$*UTF8*$!\n{"), Some(Format::OpenStep));
        assert_eq!(detect_format(b""), None);
        assert_eq!(detect_format(b"\x00\x01"), None);
    }

    #[test]
    fn openstep_is_readable_but_not_writable_yet() {
        let (v, f) = from_bytes(b"{ name = \"Hello\"; items = (a, b); }").expect("parses");
        assert_eq!(f, Format::OpenStep);
        let d = v.as_dictionary().expect("dict");
        assert_eq!(d.get("name").and_then(Value::as_string), Some("Hello"));
        assert!(matches!(to_bytes(&v, Format::OpenStep), Err(PlistError::Unsupported(_))));
    }

    #[test]
    fn garbage_is_a_parse_error() {
        assert!(matches!(from_bytes(b"bplist00garbage"), Err(PlistError::Parse(_))));
        assert!(matches!(from_bytes(b"\x00"), Err(PlistError::Parse(_))));
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        // RFC 4648 §10: the prefixes of "foobar".
        let expected = ["", "Zg==", "Zm8=", "Zm9v", "Zm9vYg==", "Zm9vYmE=", "Zm9vYmFy"];
        for (len, want) in expected.iter().enumerate() {
            assert_eq!(base64(&b"foobar"[..len]), *want);
        }
    }

    #[test]
    fn every_value_round_trips_through_both_writers() {
        let mut inner = Dictionary::new();
        inner.insert("Empty".into(), Value::Dictionary(Dictionary::new()));
        inner.insert("List".into(), Value::Array(vec![Value::from(1), Value::from(-7), Value::Boolean(false)]));
        let mut root = Dictionary::new();
        root.insert("Name".into(), Value::from("Héllo & <World>"));
        root.insert("Count".into(), Value::from(u64::MAX));
        root.insert("Ratio".into(), Value::Real(0.1));
        root.insert("Blob".into(), Value::Data((0..=255).collect()));
        root.insert("When".into(), Value::Date(plist::Date::from_xml_format("2026-10-11T07:00:00Z").expect("date")));
        root.insert("Yes".into(), Value::Boolean(true));
        root.insert("Inner".into(), Value::Dictionary(inner));
        root.insert("EmptyList".into(), Value::Array(Vec::new()));
        let value = Value::Dictionary(root);
        for format in [Format::Xml, Format::Binary] {
            let bytes = to_bytes(&value, format).expect("writes");
            let (back, detected) = from_bytes(&bytes).expect("reads");
            assert_eq!(detected, format);
            assert_eq!(back, value, "{format:?}");
            // Writing what we read gives the same bytes again.
            assert_eq!(to_bytes(&back, format).expect("writes"), bytes, "{format:?}");
        }
    }
}
