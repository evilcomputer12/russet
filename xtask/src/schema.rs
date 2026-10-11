// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask schema`: regenerates `schemas/rpc/**`, `extension/src/generated/**` and `docs/reference/rpc.md`
//! from russet-protocol's registry (ARCHITECTURE §7.7). Owner: W2.
//!
//! - `schemas/rpc/v1/russet-protocol.schema.json`: every method (class, per-call flag, confirmation,
//!   long-running flag, params and result schemas), notification and server-to-client request, the shared wire
//!   types, and their definitions in `$defs` (JSON Schema draft 2020-12).
//! - `extension/src/generated/protocol.ts`: every DTO as a TypeScript type (ts-rs, 64-bit integers as `number`)
//!   plus a typed `RussetClient` over vscode-jsonrpc's `MessageConnection`.
//! - `docs/reference/rpc.md`: the method, notification and request tables.
//!
//! `--check` regenerates in memory and fails when a committed file differs (CI's schema-drift gate). Generated
//! files are never edited or merged by hand (PLAN §0 rule 2). `schemas/rpc/CHANGELOG.md` is written by hand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use russet_protocol::{MethodInfo, NotificationInfo, TypeHooks};
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

/// Flags of `cargo xtask schema`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Fail if a generated file is stale instead of writing it.
    #[arg(long)]
    pub check: bool,
}

/// The generated files, relative to the repository root.
pub const SCHEMA_FILE: &str = "schemas/rpc/v1/russet-protocol.schema.json";
pub const TS_FILE: &str = "extension/src/generated/protocol.ts";
pub const DOC_FILE: &str = "docs/reference/rpc.md";

pub fn run(args: Args) -> Result<()> {
    let root = crate::workstreams::repo_root();
    let files = generate()?;
    let mut stale = Vec::new();
    for (rel, content) in &files {
        let path = root.join(rel);
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current == *content {
            continue;
        }
        if args.check {
            stale.push(rel.clone());
        } else {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            }
            std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
            println!("schema: wrote {rel}");
        }
    }
    if !stale.is_empty() {
        bail!("schema: stale generated files (run `cargo xtask schema`): {}", stale.join(", "));
    }
    if args.check {
        println!("schema: {} generated files are up to date", files.len());
    }
    Ok(())
}

/// Every generated file and its content.
pub fn generate() -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    out.insert(SCHEMA_FILE.to_owned(), schema_json()?);
    out.insert(TS_FILE.to_owned(), typescript()?);
    out.insert(DOC_FILE.to_owned(), reference_doc());
    Ok(out)
}

fn class_name(p: russet_protocol::Permission) -> String {
    serde_json::to_value(p).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

fn confirm_name(c: russet_protocol::Confirm) -> String {
    serde_json::to_value(c).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// Recursively sorted object keys, so the output never depends on map order.
fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let b: BTreeMap<&String, Value> = m.iter().map(|(k, v)| (k, sorted(v))).collect();
            Value::Object(b.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// Wire types that no method names directly but clients need (errors, `$/progress`, approval details).
fn extra_types() -> Vec<(&'static str, TypeHooks)> {
    use russet_protocol::approval::ApprovalRequiredDetails;
    use russet_protocol::common::{CancelParams, ProgressParams};
    use russet_protocol::initialize::{BuildMismatchDetails, ProtocolMismatchDetails};
    use russet_protocol::job::BackgroundJob;
    vec![
        ("ErrorData", TypeHooks::of::<russet_protocol::ErrorData>()),
        ("ProgressParams", TypeHooks::of::<ProgressParams>()),
        ("CancelParams", TypeHooks::of::<CancelParams>()),
        ("BackgroundJob", TypeHooks::of::<BackgroundJob>()),
        ("ApprovalRequiredDetails", TypeHooks::of::<ApprovalRequiredDetails>()),
        ("ProtocolMismatchDetails", TypeHooks::of::<ProtocolMismatchDetails>()),
        ("BuildMismatchDetails", TypeHooks::of::<BuildMismatchDetails>()),
    ]
}

fn method_entry(m: &MethodInfo, namespace: &str, g: &mut schemars::SchemaGenerator) -> Value {
    json!({
        "namespace": namespace,
        "permission": class_name(m.permission),
        "dynamicPermission": m.dynamic_permission,
        "confirm": confirm_name(m.confirm),
        "longRunning": m.long_running,
        "params": (m.params.schema)(g).to_value(),
        "result": (m.result.schema)(g).to_value(),
    })
}

fn schema_json() -> Result<String> {
    let mut g = SchemaSettings::draft2020_12().into_generator();
    let mut methods = Map::new();
    let mut notifications = Map::new();
    let mut client_methods = Map::new();
    for ns in russet_protocol::namespaces() {
        let wire = russet_protocol::namespace_of_module(ns.module);
        for m in ns.methods {
            methods.insert(m.name.to_owned(), method_entry(m, wire, &mut g));
        }
        for m in ns.client_methods {
            client_methods.insert(m.name.to_owned(), method_entry(m, wire, &mut g));
        }
        for n in ns.notifications {
            notifications
                .insert(n.name.to_owned(), json!({"namespace": wire, "params": (n.params.schema)(&mut g).to_value()}));
        }
    }
    let mut types = Map::new();
    for (name, hooks) in extra_types() {
        types.insert(name.to_owned(), (hooks.schema)(&mut g).to_value());
    }
    let defs: Map<String, Value> = g.definitions().clone().into_iter().collect();
    let doc = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "urn:russet:rpc:v1",
        "title": "russet/1: the JSON-RPC protocol of russetd (ARCHITECTURE §7-§9)",
        "x-generated": "cargo xtask schema; do not edit",
        "protocol": russet_protocol::PROTOCOL,
        "conventions": {
            "longRunning": "long-running methods also accept `progressToken` (string or integer) and `background` (boolean); with `background: true` the result is BackgroundJob ({jobId})",
            "errors": "error.data is ErrorData; error.code is the Russet number (1000-9999) or a JSON-RPC code"
        },
        "methods": Value::Object(methods),
        "notifications": Value::Object(notifications),
        "clientMethods": Value::Object(client_methods),
        "types": Value::Object(types),
        "$defs": Value::Object(defs),
    });
    let mut text = serde_json::to_string_pretty(&sorted(&doc))?;
    text.push('\n');
    Ok(text)
}

/// A scratch directory removed on drop (xtask has no temp-dir crate outside its tests).
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("russet-xtask-schema-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn collect_ts(dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_ts(&path, out)?;
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("ts") {
            continue;
        }
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_owned();
        let text = std::fs::read_to_string(&path)?;
        let body: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with("import type ") && !l.starts_with("// This file was generated by"))
            .collect();
        let body = body.join("\n").trim().to_owned();
        if let Some(previous) = out.get(&name)
            && previous != &body
        {
            bail!("two wire types are both named `{name}` in TypeScript; rename one");
        }
        out.insert(name, body);
    }
    Ok(())
}

fn typescript() -> Result<String> {
    let scratch = Scratch::new()?;
    let cfg = ts_rs::Config::new().with_out_dir(&scratch.0).with_large_int("number");
    let export = |hooks: &TypeHooks| (hooks.ts_export_all)(&cfg).map_err(|e| anyhow::anyhow!("ts-rs: {e}"));
    let mut method_types = Vec::new();
    for m in russet_protocol::registry() {
        export(&m.params)?;
        export(&m.result)?;
        method_types.push((m.name, (m.params.ts_name)(&cfg), (m.result.ts_name)(&cfg)));
    }
    let mut client_types = Vec::new();
    for m in russet_protocol::client_methods() {
        export(&m.params)?;
        export(&m.result)?;
        client_types.push((m.name, (m.params.ts_name)(&cfg), (m.result.ts_name)(&cfg)));
    }
    let mut notification_types = Vec::new();
    for n in russet_protocol::notifications() {
        export(&n.params)?;
        notification_types.push((n.name, (n.params.ts_name)(&cfg)));
    }
    for (_, hooks) in extra_types() {
        export(&hooks)?;
    }
    let mut decls = BTreeMap::new();
    collect_ts(&scratch.0, &mut decls)?;

    let mut ts = String::new();
    ts.push_str("// Generated by `cargo xtask schema` from russet-protocol (ARCHITECTURE §7.7). Do not edit.\n");
    ts.push_str("// The russet/1 wire types and a typed client over vscode-jsonrpc.\n\n");
    ts.push_str("import type { CancellationToken, Disposable, MessageConnection } from \"vscode-jsonrpc\";\n\n");
    let p = russet_protocol::PROTOCOL;
    ts.push_str(&format!("export const PROTOCOL = {{ major: {}, minor: {} }} as const;\n\n", p.major, p.minor));
    for body in decls.values() {
        ts.push_str(body);
        ts.push_str("\n\n");
    }
    ts.push_str("/** Every method the daemon serves: its params and result types. */\nexport interface Methods {\n");
    for (name, params, result) in &method_types {
        ts.push_str(&format!("  \"{name}\": {{ params: {params}; result: {result} }};\n"));
    }
    ts.push_str("}\n\n/** Every notification the daemon sends. */\nexport interface Notifications {\n");
    for (name, params) in &notification_types {
        ts.push_str(&format!("  \"{name}\": {params};\n"));
    }
    ts.push_str(
        "}\n\n/** Every request the daemon sends to clients (`ui.*`, `ide.*`). */\nexport interface ClientMethods {\n",
    );
    for (name, params, result) in &client_types {
        ts.push_str(&format!("  \"{name}\": {{ params: {params}; result: {result} }};\n"));
    }
    ts.push_str("}\n\n");
    ts.push_str(CLIENT_CLASS);
    Ok(ts)
}

const CLIENT_CLASS: &str = r#"/** Long-running methods also accept these (ARCHITECTURE §7.6). */
export interface LongRunningOptions {
  progressToken?: string | number;
  /** Return `{jobId}` at once; the job survives this connection. */
  background?: boolean;
}

/** A typed russet/1 client over a vscode-jsonrpc connection (ARCHITECTURE §7.2). */
export class RussetClient {
  readonly connection: MessageConnection;

  constructor(connection: MessageConnection) {
    this.connection = connection;
  }

  /** Calls `method`; rejects with a ResponseError whose `data` is ErrorData. */
  request<M extends keyof Methods>(
    method: M,
    params: Methods[M]["params"] & LongRunningOptions,
    token?: CancellationToken,
  ): Promise<Methods[M]["result"]> {
    return this.connection.sendRequest(method, params, token) as Promise<Methods[M]["result"]>;
  }

  /** Calls a long-running `method` with `background: true`: resolves to `{jobId}` at once. */
  background<M extends keyof Methods>(method: M, params: Methods[M]["params"]): Promise<BackgroundJob> {
    return this.connection.sendRequest(method, { ...params, background: true }) as Promise<BackgroundJob>;
  }

  /** Receives a notification. */
  onNotification<N extends keyof Notifications>(method: N, handler: (params: Notifications[N]) => void): Disposable {
    return this.connection.onNotification(method, handler);
  }

  /** Answers a request of the daemon (`ui.confirm`, `ui.prompt`, `ui.openUrl`, `ide.*`). */
  onRequest<M extends keyof ClientMethods>(
    method: M,
    handler: (params: ClientMethods[M]["params"]) => ClientMethods[M]["result"] | Promise<ClientMethods[M]["result"]>,
  ): Disposable {
    return this.connection.onRequest(method, handler);
  }
}
"#;

fn reference_doc() -> String {
    let mut doc = String::new();
    doc.push_str("# The russet/1 method reference\n\n");
    doc.push_str(
        "Generated by `cargo xtask schema` from russet-protocol's registry (ARCHITECTURE §7.7); do not edit.\n",
    );
    doc.push_str(
        "The schemas are in `schemas/rpc/v1/russet-protocol.schema.json`; ARCHITECTURE §8 explains every method.\n\n",
    );
    let p = russet_protocol::PROTOCOL;
    doc.push_str(&format!("Protocol version {}.{}.\n\n", p.major, p.minor));
    doc.push_str("- **Class:** the permission class (ARCHITECTURE §7.5); `per call` rows resolve it from the params or the caller.\n");
    doc.push_str("- **L:** long-running: the call is a job, accepts `progressToken` and `background`.\n");
    doc.push_str("- **Confirm:** `typed` needs a typed confirmation (`--confirm-text`).\n\n");
    for ns in russet_protocol::namespaces() {
        let cfg = ts_rs::Config::new().with_large_int("number");
        if ns.methods.is_empty() && ns.notifications.is_empty() && ns.client_methods.is_empty() {
            continue;
        }
        doc.push_str(&format!("## `{}` (module `{}`)\n\n", russet_protocol::namespace_of_module(ns.module), ns.module));
        let method_rows = |doc: &mut String, title: &str, list: &[MethodInfo]| {
            if list.is_empty() {
                return;
            }
            doc.push_str(&format!(
                "{title}\n\n| Method | Class | L | Confirm | Params | Result |\n|---|---|---|---|---|---|\n"
            ));
            for m in list {
                let class = if m.dynamic_permission {
                    format!("{} (per call)", class_name(m.permission))
                } else {
                    class_name(m.permission)
                };
                doc.push_str(&format!(
                    "| `{}` | {} | {} | {} | `{}` | `{}` |\n",
                    m.name,
                    class,
                    if m.long_running { "L" } else { "" },
                    confirm_name(m.confirm),
                    (m.params.ts_name)(&cfg),
                    (m.result.ts_name)(&cfg)
                ));
            }
            doc.push('\n');
        };
        method_rows(&mut doc, "Methods:", ns.methods);
        method_rows(&mut doc, "Requests to clients:", ns.client_methods);
        if !ns.notifications.is_empty() {
            doc.push_str("Notifications:\n\n| Notification | Params |\n|---|---|\n");
            for n in ns.notifications {
                let NotificationInfo { name, params } = n;
                doc.push_str(&format!("| `{name}` | `{}` |\n", (params.ts_name)(&cfg)));
            }
            doc.push('\n');
        }
    }
    doc.trim_end().to_owned() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema snapshot (PLAN W2 tests): the committed generated files are what the registry produces now.
    #[test]
    fn the_committed_generated_files_are_up_to_date() {
        let root = crate::workstreams::repo_root();
        for (rel, content) in generate().expect("generates") {
            let committed = std::fs::read_to_string(root.join(&rel)).unwrap_or_default();
            assert!(committed == content, "{rel} is stale: run `cargo xtask schema`");
        }
    }

    #[test]
    fn the_schema_describes_the_core_methods() {
        let v: Value = serde_json::from_str(&schema_json().expect("schema")).expect("JSON");
        let status = &v["methods"]["daemon.status"];
        assert_eq!(status["permission"], "read");
        assert_eq!(status["result"]["$ref"], "#/$defs/DaemonStatusResult");
        assert!(v["$defs"]["DaemonStatusResult"]["properties"]["buildId"].is_object());
        assert_eq!(v["methods"]["job.cancel"]["dynamicPermission"], true);
        assert!(v["clientMethods"]["ui.confirm"].is_object());
        assert!(v["notifications"]["job.events"].is_object());
        assert!(v["types"]["ErrorData"].is_object());
    }

    #[test]
    fn the_typescript_has_the_client_and_the_types() {
        let ts = typescript().expect("ts");
        for needle in [
            "export class RussetClient",
            "\"daemon.status\": { params: DaemonStatusParams; result: DaemonStatusResult };",
            "export type DaemonStatusResult",
            "export type JobEvent",
            "\"ui.confirm\": { params: UiConfirmParams; result: UiConfirmResult };",
        ] {
            assert!(ts.contains(needle), "{needle} missing");
        }
        assert!(
            !ts.contains("import type {") || ts.matches("import type").count() == 1,
            "only the vscode-jsonrpc import"
        );
        assert!(!ts.contains("bigint"), "64-bit integers are numbers");
    }
}
