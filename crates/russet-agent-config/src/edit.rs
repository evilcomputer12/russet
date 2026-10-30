// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Comment-preserving edits to JSON(C) and TOML config files, and the atomic-write-with-backup used by
//! [`crate::apply`].

use std::io;
use std::path::Path;

use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstInputValue, CstRootNode};
use serde_json::Value;

/// A `serde_json::Value` as a `jsonc-parser` CST input value (numbers keep their JSON spelling).
pub fn json_to_cst(value: &Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(*b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s.clone()),
        Value::Array(a) => CstInputValue::Array(a.iter().map(json_to_cst).collect()),
        Value::Object(o) => CstInputValue::Object(o.iter().map(|(k, v)| (k.clone(), json_to_cst(v))).collect()),
    }
}

/// Sets `parent_path → key = value` in the JSON(C) `text`, creating intermediate objects, preserving comments
/// and formatting. Returns the new text, or `None` when the file already has exactly this value (idempotent).
///
/// `parent_path` is the chain of object keys from the root to the object that holds `key` (for example
/// `["mcpServers"]` or `["servers"]`).
pub fn set_nested_json(text: &str, parent_path: &[&str], key: &str, value: &Value) -> Result<Option<String>, String> {
    let root = CstRootNode::parse(text, &ParseOptions::default()).map_err(|e| e.to_string())?;
    let mut object = root.object_value_or_set();
    for segment in parent_path {
        object = object
            .object_value_or_create(segment)
            .ok_or_else(|| format!("`{segment}` in the config file is not a JSON object"))?;
    }
    if let Some(prop) = object.get(key) {
        prop.set_value(json_to_cst(value));
    } else {
        object.append(key, json_to_cst(value));
    }
    let after = root.to_string();
    // Idempotent: re-applying the same registration leaves the file byte-for-byte unchanged.
    if after == text { Ok(None) } else { Ok(Some(after)) }
}

/// Removes `parent_path → key` from the JSON(C) `text`. Returns the new text, or `None` when the key is
/// already absent.
pub fn remove_nested_json(text: &str, parent_path: &[&str], key: &str) -> Result<Option<String>, String> {
    let root = CstRootNode::parse(text, &ParseOptions::default()).map_err(|e| e.to_string())?;
    let Some(mut object) = root.object_value() else { return Ok(None) };
    for segment in parent_path {
        match object.object_value(segment) {
            Some(inner) => object = inner,
            None => return Ok(None),
        }
    }
    match object.get(key) {
        Some(prop) => prop.remove(),
        None => return Ok(None),
    }
    Ok(Some(root.to_string()))
}

/// Writes `after` to `path`, first backing up any existing file to `<path>.russet-bak`, creating parent
/// directories, and writing atomically. `before` is passed for symmetry with [`crate::Change`]; the backup is
/// taken from the file on disk.
pub fn write_atomic_with_backup(path: &Path, _before: Option<&str>, after: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if path.exists() {
        let backup = backup_path(path);
        std::fs::copy(path, &backup)?;
    }
    russet_core::fsx::atomic_write(path, after.as_bytes()).map_err(|e| io::Error::other(e.to_string()))
}

/// The backup path for `path`: the same path with `.russet-bak` appended.
pub fn backup_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".russet-bak");
    std::path::PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn creates_nested_object_in_empty_file() {
        let out = set_nested_json("{}", &["mcpServers"], "russet", &json!({"command": "russet"})).unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["russet"]["command"], "russet");
    }

    #[test]
    fn preserves_comments_and_siblings() {
        let text = "{\n  // keep me\n  \"other\": {\"mcpServers\": {}},\n  \"mcpServers\": {\n    \"existing\": {\"command\": \"x\"}\n  }\n}";
        let out = set_nested_json(text, &["mcpServers"], "russet", &json!({"command": "russet"})).unwrap().unwrap();
        assert!(out.contains("// keep me"), "comment preserved");
        assert!(out.contains("\"existing\""), "sibling preserved");
        let v: Value = serde_json::from_str(&strip_comments(&out)).unwrap();
        assert_eq!(v["mcpServers"]["russet"]["command"], "russet");
        assert_eq!(v["mcpServers"]["existing"]["command"], "x");
    }

    #[test]
    fn idempotent_when_value_matches() {
        let text = set_nested_json("{}", &["mcpServers"], "russet", &json!({"command": "russet"})).unwrap().unwrap();
        let again = set_nested_json(&text, &["mcpServers"], "russet", &json!({"command": "russet"})).unwrap();
        assert_eq!(again, None, "no change the second time");
    }

    #[test]
    fn remove_is_idempotent() {
        let text = set_nested_json("{}", &["mcpServers"], "russet", &json!({"command": "russet"})).unwrap().unwrap();
        let removed = remove_nested_json(&text, &["mcpServers"], "russet").unwrap().unwrap();
        let v: Value = serde_json::from_str(&removed).unwrap();
        assert!(v["mcpServers"].get("russet").is_none());
        assert_eq!(remove_nested_json(&removed, &["mcpServers"], "russet").unwrap(), None);
    }

    fn strip_comments(s: &str) -> String {
        s.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
    }
}
