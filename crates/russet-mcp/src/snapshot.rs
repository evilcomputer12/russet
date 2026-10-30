// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The MCP tool-catalog snapshot (`schemas/mcp/tools.v1.json`), the contract snapshot of ARCHITECTURE §11.4
//! and the F4 freeze artifact (PLAN §10). It is exactly what `tools/list` returns, plus each tool's tier and
//! RPC method map.
//!
//! The committed file is drift-checked by [`tests::snapshot_is_current`]. To regenerate it after an
//! intentional catalog change, run the test with `RUSSET_BLESS=1`:
//! `RUSSET_BLESS=1 cargo test -p russet-mcp snapshot_is_current`.

use serde_json::{Map, Value, json};

use crate::catalog::{SCHEMA_VERSION, tool_catalog};

/// The snapshot path, relative to the repository root.
pub const SNAPSHOT_FILE: &str = "schemas/mcp/tools.v1.json";

/// Recursively key-sorted JSON so the output never depends on map order.
fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let b: std::collections::BTreeMap<&String, Value> = m.iter().map(|(k, v)| (k, sorted(v))).collect();
            Value::Object(b.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// The catalog as a JSON document, optionally filtered to tools of tier ≤ `tier_max` (key-sorted). `None`
/// includes every tool. This is what `russet mcp tools --json` prints and what [`render`] freezes.
pub fn catalog_json(tier_max: Option<u8>) -> Value {
    let tools: Vec<Value> = tool_catalog()
        .iter()
        .filter(|t| tier_max.is_none_or(|mx| t.tier <= mx))
        .map(|t| {
            let mut entry = serde_json::to_value(&t.tool).unwrap_or_else(|_| json!({}));
            if let Value::Object(map) = &mut entry {
                map.insert("tier".to_owned(), json!(t.tier));
                map.insert(
                    "rpc".to_owned(),
                    Value::Array(t.method_map.iter().map(|m| Value::String((*m).to_owned())).collect()),
                );
            }
            entry
        })
        .collect();
    let mut doc = Map::new();
    doc.insert("$schema-version".to_owned(), json!(SCHEMA_VERSION));
    doc.insert(
        "x-generated".to_owned(),
        json!("russet-mcp::snapshot (ARCHITECTURE §11.4); regenerate with RUSSET_BLESS=1 cargo test -p russet-mcp snapshot_is_current"),
    );
    doc.insert("tools".to_owned(), Value::Array(tools));
    sorted(&Value::Object(doc))
}

/// The snapshot content (a trailing newline, like the other generated files).
pub fn render() -> String {
    let mut text = serde_json::to_string_pretty(&catalog_json(None)).expect("snapshot json");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn snapshot_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(SNAPSHOT_FILE)
    }

    #[test]
    fn snapshot_is_current() {
        let content = render();
        let path = snapshot_path();
        if std::env::var_os("RUSSET_BLESS").is_some() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(&path, &content).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == content,
            "{SNAPSHOT_FILE} is stale: run `RUSSET_BLESS=1 cargo test -p russet-mcp snapshot_is_current`"
        );
    }

    #[test]
    fn snapshot_lists_every_tool_with_meta() {
        let v: Value = serde_json::from_str(&render()).unwrap();
        let tools = v["tools"].as_array().unwrap();
        assert_eq!(tools.len(), tool_catalog().len());
        for t in tools {
            assert!(t["name"].is_string());
            assert!(t["tier"].is_u64());
            assert!(t["rpc"].is_array());
            assert_eq!(t["_meta"]["x-russet-schema"], json!(SCHEMA_VERSION));
        }
    }
}
