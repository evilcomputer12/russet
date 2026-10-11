// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! JSON Schemas of the configuration files (`config.schema`, `schemas/config/*.schema.json`) and
//! schema-directed helpers for `config set`: does a key exist, and what JSON type does it take.

use schemars::generate::SchemaSettings;
use serde_json::Value;

use crate::accounts::AccountsFile;
use crate::global::GlobalConfig;
use crate::load::ConfigFile;
use crate::policy::{McpPolicy, ProvisionPolicy};
use crate::project::ProjectConfig;

/// The JSON Schema (draft 2020-12) of one configuration file.
pub fn schema(file: ConfigFile) -> Value {
    let generator = SchemaSettings::draft2020_12().into_generator();
    let schema = match file {
        ConfigFile::Project => generator.into_root_schema_for::<ProjectConfig>(),
        ConfigFile::Global => generator.into_root_schema_for::<GlobalConfig>(),
        ConfigFile::Accounts => generator.into_root_schema_for::<AccountsFile>(),
        ConfigFile::McpPolicy => generator.into_root_schema_for::<McpPolicy>(),
        ConfigFile::ProvisionPolicy => generator.into_root_schema_for::<ProvisionPolicy>(),
    };
    let mut v = schema.to_value();
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "$id".into(),
            Value::String(format!("https://russet.dev/schemas/config/{}.schema.json", file.as_str())),
        );
    }
    v
}

/// The committed file name of a schema: `schemas/config/<kind>.schema.json`.
pub fn schema_file_name(file: ConfigFile) -> String {
    format!("{}.schema.json", file.as_str())
}

/// Follows `$ref` (into `$defs`) until `s` is a concrete schema.
fn resolve<'a>(root: &'a Value, mut s: &'a Value) -> &'a Value {
    for _ in 0..16 {
        let Some(r) = s.get("$ref").and_then(Value::as_str) else { return s };
        let Some(name) = r.strip_prefix("#/$defs/") else { return s };
        match root.get("$defs").and_then(|d| d.get(name)) {
            Some(next) => s = next,
            None => return s,
        }
    }
    s
}

/// The alternatives of a schema (`anyOf`/`oneOf` branches, or itself), resolved and without `null`.
fn branches<'a>(root: &'a Value, s: &'a Value) -> Vec<&'a Value> {
    let s = resolve(root, s);
    let mut out = Vec::new();
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(items)) = s.get(key) {
            for b in items {
                let b = resolve(root, b);
                if b.get("type").and_then(Value::as_str) != Some("null") {
                    out.extend(branches(root, b));
                }
            }
        }
    }
    if out.is_empty() {
        out.push(s);
    }
    out
}

/// The schema of the value at `segments`, or `None` when the key does not exist in the file.
pub fn schema_at<'a>(root: &'a Value, segments: &[String]) -> Option<&'a Value> {
    let mut current = vec![root];
    for seg in segments {
        let mut next = Vec::new();
        for s in current.iter().flat_map(|s| branches(root, s)) {
            if let Some(p) = s.get("properties").and_then(|p| p.get(seg)) {
                next.push(p);
            } else if let Some(ap) = s.get("additionalProperties")
                && ap.is_object()
            {
                next.push(ap);
            }
        }
        if next.is_empty() {
            return None;
        }
        current = next;
    }
    current.first().copied()
}

/// The keys allowed directly below `segments` (for "did you mean" hints).
pub fn keys_at(root: &Value, segments: &[String]) -> Vec<String> {
    let Some(s) = (if segments.is_empty() { Some(root) } else { schema_at(root, segments) }) else {
        return Vec::new();
    };
    let mut keys: Vec<String> = branches(root, s)
        .into_iter()
        .filter_map(|b| b.get("properties").and_then(Value::as_object))
        .flat_map(|p| p.keys().cloned())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

fn type_matches(ty: &str, v: &Value) -> bool {
    match ty {
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "null" => v.is_null(),
        _ => true,
    }
}

/// A shallow check: does `v` have a type (and constant or enumerated value) that `s` allows?
fn accepts(root: &Value, s: &Value, v: &Value) -> bool {
    branches(root, s).into_iter().any(|b| {
        if let Some(c) = b.get("const") {
            return c == v;
        }
        if let Some(Value::Array(allowed)) = b.get("enum") {
            return allowed.contains(v);
        }
        match b.get("type") {
            Some(Value::String(t)) => type_matches(t, v),
            Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).any(|t| type_matches(t, v)),
            _ => true,
        }
    })
}

/// Converts `value` to the JSON type the schema expects, when a lossless conversion exists: `"2"` → `2` for
/// integers, `"true"` → `true`, `1` → `"1"` for strings, `"-Onone"` → `["-Onone"]` for string arrays, and JSON
/// text for arrays and tables. Anything else is returned unchanged, and validation reports it.
pub fn coerce(root: &Value, schema: &Value, value: Value) -> Value {
    if accepts(root, schema, &value) {
        return value;
    }
    let mut candidates: Vec<Value> = Vec::new();
    match &value {
        Value::String(s) => {
            let t = s.trim();
            if let Ok(i) = t.parse::<i64>() {
                candidates.push(Value::from(i));
            }
            if let Ok(f) = t.parse::<f64>()
                && let Some(n) = serde_json::Number::from_f64(f)
            {
                candidates.push(Value::Number(n));
            }
            match t {
                "true" => candidates.push(Value::Bool(true)),
                "false" => candidates.push(Value::Bool(false)),
                _ => {}
            }
            if let Ok(parsed) = serde_json::from_str::<Value>(t) {
                candidates.push(parsed);
            }
            candidates.push(Value::Array(vec![value.clone()]));
        }
        Value::Number(n) => candidates.push(Value::String(n.to_string())),
        Value::Bool(b) => candidates.push(Value::String(b.to_string())),
        _ => {}
    }
    candidates.into_iter().find(|c| accepts(root, schema, c)).unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(k: &str) -> Vec<String> {
        crate::keys::parse(k).expect("key")
    }

    #[test]
    fn every_file_has_a_schema_with_its_sections() {
        let g = schema(ConfigFile::Global);
        assert!(schema_at(&g, &segs("daemon.idle-timeout")).is_some());
        assert!(schema_at(&g, &segs("device.ddi.dir")).is_some());
        assert!(schema_at(&g, &segs("devices.aliases.iphone12")).is_some(), "map keys resolve");
        assert!(schema_at(&g, &segs("daemon.idle-timeouts")).is_none());
        let p = schema(ConfigFile::Project);
        assert!(schema_at(&p, &segs("signing.targets.\"PlugIns/W.appex\".profile")).is_some());
        assert!(schema_at(&p, &segs("agents.tier-max")).is_some());
        assert!(schema_at(&p, &segs("build.flutter.target")).is_some(), "optional tables resolve");
        for f in ConfigFile::ALL {
            let s = schema(f);
            assert_eq!(s["$schema"], "https://json-schema.org/draft/2020-12/schema", "{f:?}");
        }
        assert!(keys_at(&g, &segs("daemon")).contains(&"idle-timeout".to_owned()));
    }

    #[test]
    fn values_are_coerced_to_the_schema_type() {
        let g = schema(ConfigFile::Global);
        let jobs = schema_at(&g, &segs("daemon.max-build-jobs")).expect("jobs");
        assert_eq!(coerce(&g, jobs, Value::from("2")), Value::from(2));
        let filter = schema_at(&g, &segs("daemon.log-filter")).expect("filter");
        assert_eq!(coerce(&g, filter, Value::from(7)), Value::from("7"));
        let allow = schema_at(&g, &segs("device.ddi.allow-mirror")).expect("bool");
        assert_eq!(coerce(&g, allow, Value::from("true")), Value::Bool(true));
        let order = schema_at(&g, &segs("device.ddi.order")).expect("order");
        assert_eq!(coerce(&g, order, Value::from("[\"dir\"]")), serde_json::json!(["dir"]));
        let p = schema(ConfigFile::Project);
        let build_number = schema_at(&p, &segs("app.build-number")).expect("build-number");
        assert_eq!(coerce(&p, build_number, Value::from(1)), Value::from("1"));
        let jobs = schema_at(&p, &segs("build.jobs")).expect("jobs");
        assert_eq!(coerce(&p, jobs, Value::from("4")), Value::from(4));
        assert_eq!(coerce(&p, jobs, Value::from("auto")), Value::from("auto"));
        let flags = schema_at(&p, &segs("build.swiftpm.swiftc-flags")).expect("flags");
        assert_eq!(coerce(&p, flags, Value::from("-Onone")), serde_json::json!(["-Onone"]));
        let install = schema_at(&p, &segs("run.install")).expect("install");
        assert_eq!(coerce(&p, install, Value::from("sometimes")), Value::from("sometimes"), "left for validation");
    }
}
