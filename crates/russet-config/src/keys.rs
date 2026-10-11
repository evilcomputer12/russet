// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Dotted keys in TOML syntax (`daemon.idle-timeout`, `signing.targets."PlugIns/W.appex".profile`) and lookups
//! by key in JSON values and TOML documents.

use std::ops::Range;

use russet_core::error::{Error, Result, codes};

/// Splits a dotted key written in TOML syntax into its segments.
pub fn parse(key: &str) -> Result<Vec<String>> {
    if key.trim().is_empty() {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, "empty configuration key")
            .with_hint("Name a key such as `daemon.idle-timeout`."));
    }
    let parsed = toml_edit::Key::parse(key).map_err(|e| {
        Error::new(codes::core::INVALID_ARGUMENT, format!("`{key}` is not a valid dotted key: {}", e.message().trim()))
            .with_hint("Quote segments that contain dots or slashes: signing.targets.\"PlugIns/W.appex\".profile")
    })?;
    Ok(parsed.iter().map(|k| k.get().to_owned()).collect())
}

/// Writes segments back as a dotted key, quoting where TOML needs it.
pub fn render(segments: &[String]) -> String {
    segments.iter().map(|s| toml_edit::Key::new(s.as_str()).display_repr().into_owned()).collect::<Vec<_>>().join(".")
}

/// The value at `segments` inside a JSON value.
pub fn get_json<'a>(root: &'a serde_json::Value, segments: &[String]) -> Option<&'a serde_json::Value> {
    segments.iter().try_fold(root, |v, s| v.as_object()?.get(s))
}

/// Whether the raw TOML table defines `segments`.
pub fn toml_has(root: &toml::Table, segments: &[String]) -> bool {
    let Some((last, parents)) = segments.split_last() else { return false };
    let mut t = root;
    for s in parents {
        match t.get(s) {
            Some(toml::Value::Table(next)) => t = next,
            _ => return false,
        }
    }
    t.contains_key(last)
}

/// The byte span of the value (or table) at `segments` in TOML `text`, for error positions.
pub fn span_of(text: &str, segments: &[&str]) -> Option<Range<usize>> {
    let doc = toml_edit::Document::parse(text).ok()?;
    let mut item = doc.as_item();
    for s in segments {
        item = item.get(s)?;
    }
    item.span()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_keys_round_trip_with_quotes() {
        assert_eq!(parse("daemon.idle-timeout").ok(), Some(vec!["daemon".to_owned(), "idle-timeout".to_owned()]));
        let segs = parse("signing.targets.\"PlugIns/W.appex\".profile").expect("quoted");
        assert_eq!(segs, vec!["signing", "targets", "PlugIns/W.appex", "profile"]);
        assert_eq!(render(&segs), "signing.targets.\"PlugIns/W.appex\".profile");
        assert!(parse("").is_err());
        assert!(parse("a..b").is_err());
    }

    #[test]
    fn lookups_and_spans() {
        let v = serde_json::json!({"a": {"b": 1}});
        assert_eq!(get_json(&v, &["a".into(), "b".into()]), Some(&serde_json::json!(1)));
        assert_eq!(get_json(&v, &["a".into(), "c".into()]), None);
        let t: toml::Table = toml::from_str("[a]\nb = 1\n").expect("toml");
        assert!(toml_has(&t, &["a".into(), "b".into()]));
        assert!(!toml_has(&t, &["a".into(), "c".into()]));
        assert_eq!(span_of("x = 1\n[a]\nb = 22\n", &["a", "b"]), Some(14..16));
    }
}
