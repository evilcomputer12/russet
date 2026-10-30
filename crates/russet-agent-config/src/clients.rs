// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Per-client registration targets and detection (IDE §6.6 table). W13 implements Claude Code, Codex, Gemini
//! and VS Code (MCP-04 partial).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::edit;
use crate::{AgentClient, Change, PlanError, Registration, Scope};

/// What [`crate::detect`] found about one client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedState {
    /// The client appears to be installed (its config directory exists or its CLI is on PATH).
    pub installed: bool,
    /// The client's own CLI is on PATH (`claude`, `codex`, `gemini`, `code`).
    pub cli_available: bool,
    /// Russet is registered in the user-scope file.
    pub registered: bool,
    /// The user-scope file inspected.
    pub path: Option<PathBuf>,
}

/// The file format a client's config uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// JSON with comments; the server entry lives under `parent` → `russet`.
    Jsonc,
    /// TOML; the server entry is the table `mcp_servers.russet`.
    Toml,
}

/// A resolved registration target: the file, its format and the full server entry to write.
struct Target {
    path: PathBuf,
    format: Format,
    /// The JSON(C) parent path to the object that holds the `russet` key (`["mcpServers"]`, `["servers"]`).
    json_parent: Vec<&'static str>,
    /// The entry value (`{command, args, …}`).
    entry: Value,
}

/// The CLI the client offers for `mcp add`, for [`Change::via_cli`] (informational).
fn via_cli(reg: &Registration) -> Option<Vec<String>> {
    let cmd = reg.command.to_string_lossy().into_owned();
    let id = reg.client.id();
    let v = match reg.client {
        AgentClient::ClaudeCode => {
            vec!["claude", "mcp", "add", "-s", scope_flag(&reg.scope), "russet", "--", &cmd]
        }
        AgentClient::Codex => vec!["codex", "mcp", "add", "russet", "--", &cmd],
        AgentClient::Gemini => vec!["gemini", "mcp", "add", "-s", scope_flag(&reg.scope), "russet", &cmd],
        _ => return None,
    };
    let mut out: Vec<String> = v.into_iter().map(str::to_owned).collect();
    out.extend(reg.args.iter().cloned());
    // The server id argument differs per client; keep the client id visible for clarity.
    let _ = id;
    Some(out)
}

fn scope_flag(scope: &Scope) -> &'static str {
    match scope {
        Scope::User => "user",
        Scope::Project(_) => "project",
    }
}

/// The `{command, args, …extra}` entry for a stdio server.
fn stdio_entry(reg: &Registration, extra: &[(&str, Value)]) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("command".to_owned(), Value::String(reg.command.to_string_lossy().into_owned()));
    map.insert("args".to_owned(), Value::Array(reg.args.iter().map(|a| Value::String(a.clone())).collect()));
    for (k, v) in extra {
        map.insert((*k).to_owned(), v.clone());
    }
    Value::Object(map)
}

/// The home directory (`HOME`, or `USERPROFILE` on Windows).
fn home() -> Result<PathBuf, PlanError> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(PlanError::NoHome)
}

/// Resolves the registration target for `reg`.
fn target(reg: &Registration) -> Result<Target, PlanError> {
    match reg.client {
        AgentClient::ClaudeCode => {
            let path = match &reg.scope {
                Scope::User => home()?.join(".claude.json"),
                Scope::Project(dir) => dir.join(".mcp.json"),
            };
            Ok(Target { path, format: Format::Jsonc, json_parent: vec!["mcpServers"], entry: stdio_entry(reg, &[]) })
        }
        AgentClient::Codex => {
            let path = match &reg.scope {
                Scope::User => home()?.join(".codex").join("config.toml"),
                Scope::Project(dir) => dir.join(".codex").join("config.toml"),
            };
            let entry = stdio_entry(reg, &[("startup_timeout_sec", json!(20)), ("tool_timeout_sec", json!(300))]);
            Ok(Target { path, format: Format::Toml, json_parent: vec!["mcp_servers"], entry })
        }
        AgentClient::Gemini => {
            let path = match &reg.scope {
                Scope::User => home()?.join(".gemini").join("settings.json"),
                Scope::Project(dir) => dir.join(".gemini").join("settings.json"),
            };
            let entry = stdio_entry(reg, &[("timeout", json!(600000)), ("trust", json!(false))]);
            Ok(Target { path, format: Format::Jsonc, json_parent: vec!["mcpServers"], entry })
        }
        AgentClient::VsCode => {
            let dir = match &reg.scope {
                Scope::Project(dir) => dir,
                Scope::User => {
                    return Err(PlanError::ScopeUnsupported {
                        client: AgentClient::VsCode,
                        scope: "user",
                        hint: "VS Code user-scope registration is the extension's MCP API (IDE §6.9); use project scope or the Russet IDE",
                    });
                }
            };
            let entry = stdio_entry(reg, &[("type", json!("stdio"))]);
            Ok(Target {
                path: dir.join(".vscode").join("mcp.json"),
                format: Format::Jsonc,
                json_parent: vec!["servers"],
                entry,
            })
        }
        other => Err(PlanError::Unsupported(other)),
    }
}

/// Plans installing `reg` (see [`crate::plan`]).
pub fn plan_install(reg: &Registration) -> Result<Vec<Change>, PlanError> {
    let t = target(reg)?;
    let before = read_existing(&t.path)?;
    let text = before.clone().unwrap_or_else(|| default_doc(t.format));
    let after = match t.format {
        Format::Jsonc => {
            edit::set_nested_json(&text, &t.json_parent, &reg.server_name, &t.entry).map_err(PlanError::Io)?
        }
        Format::Toml => set_toml_entry(&text, &reg.server_name, &t.entry).map_err(PlanError::Io)?,
    };
    Ok(match after {
        None => Vec::new(),
        Some(after) => vec![Change { path: Some(t.path), via_cli: via_cli(reg), before, after }],
    })
}

/// Plans uninstalling `reg` (see [`crate::plan_uninstall`]).
pub fn plan_uninstall(reg: &Registration) -> Result<Vec<Change>, PlanError> {
    let t = target(reg)?;
    let Some(before) = read_existing(&t.path)? else { return Ok(Vec::new()) };
    let after = match t.format {
        Format::Jsonc => edit::remove_nested_json(&before, &t.json_parent, &reg.server_name).map_err(PlanError::Io)?,
        Format::Toml => remove_toml_entry(&before, &reg.server_name).map_err(PlanError::Io)?,
    };
    Ok(match after {
        None => Vec::new(),
        Some(after) => vec![Change { path: Some(t.path), via_cli: None, before: Some(before), after }],
    })
}

/// Detects one client.
pub fn detect_one(client: AgentClient) -> DetectedState {
    let user_reg = Registration::stdio(client, Scope::User, "russet");
    let (path, registered) = match target(&user_reg) {
        Ok(t) => {
            let registered = read_existing(&t.path)
                .ok()
                .flatten()
                .map(|text| entry_present(&text, t.format, &t.json_parent, "russet"))
                .unwrap_or(false);
            (Some(t.path), registered)
        }
        // A client without a user-scope file (VS Code) is detected only by its CLI / config dir.
        Err(_) => (None, false),
    };
    let cli_available = cli_name(client).map(on_path).unwrap_or(false);
    let installed = cli_available || config_dir_exists(client);
    DetectedState { installed, cli_available, registered, path }
}

fn entry_present(text: &str, format: Format, json_parent: &[&str], key: &str) -> bool {
    match format {
        Format::Jsonc => {
            let Ok(root) = jsonc_parser::cst::CstRootNode::parse(text, &jsonc_parser::ParseOptions::default()) else {
                return false;
            };
            let Some(mut object) = root.object_value() else { return false };
            for seg in json_parent {
                match object.object_value(seg) {
                    Some(inner) => object = inner,
                    None => return false,
                }
            }
            object.get(key).is_some()
        }
        Format::Toml => text
            .parse::<toml_edit::DocumentMut>()
            .ok()
            .map(|doc| doc.get("mcp_servers").and_then(|t| t.get(key)).is_some())
            .unwrap_or(false),
    }
}

fn read_existing(path: &Path) -> Result<Option<String>, PlanError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PlanError::Io(format!("reading {}: {e}", path.display()))),
    }
}

fn default_doc(format: Format) -> String {
    match format {
        Format::Jsonc => "{}\n".to_owned(),
        Format::Toml => String::new(),
    }
}

/// Sets `[mcp_servers.<key>]` from the JSON `entry`, preserving the rest of the TOML. `None` when unchanged.
fn set_toml_entry(text: &str, key: &str, entry: &Value) -> Result<Option<String>, String> {
    let mut doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| e.to_string())?;
    let desired = json_to_toml_table(entry)?;
    let existing = doc.get("mcp_servers").and_then(|t| t.get(key));
    if existing.and_then(|i| i.as_table()).map(|t| table_eq(t, &desired)).unwrap_or(false) {
        return Ok(None);
    }
    let servers = doc
        .entry("mcp_servers")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("`mcp_servers` is not a TOML table")?;
    servers.set_implicit(true);
    servers.insert(key, toml_edit::Item::Table(desired));
    Ok(Some(doc.to_string()))
}

/// Removes `[mcp_servers.<key>]`. `None` when absent.
fn remove_toml_entry(text: &str, key: &str) -> Result<Option<String>, String> {
    let mut doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| e.to_string())?;
    let Some(servers) = doc.get_mut("mcp_servers").and_then(|t| t.as_table_mut()) else {
        return Ok(None);
    };
    if servers.remove(key).is_none() {
        return Ok(None);
    }
    Ok(Some(doc.to_string()))
}

/// A JSON object as a TOML table (strings, integers, booleans and arrays of strings; enough for the server
/// entries of IDE §6.6).
fn json_to_toml_table(entry: &Value) -> Result<toml_edit::Table, String> {
    let obj = entry.as_object().ok_or("the server entry is not an object")?;
    let mut table = toml_edit::Table::new();
    for (k, v) in obj {
        table.insert(k, toml_edit::Item::Value(json_to_toml_value(v)?));
    }
    Ok(table)
}

fn json_to_toml_value(v: &Value) -> Result<toml_edit::Value, String> {
    Ok(match v {
        Value::String(s) => s.as_str().into(),
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else if let Some(u) = n.as_u64().and_then(|u| i64::try_from(u).ok()) {
                u.into()
            } else if let Some(f) = n.as_f64() {
                f.into()
            } else {
                return Err(format!("number {n} does not fit a TOML value"));
            }
        }
        Value::Array(a) => {
            let mut arr = toml_edit::Array::new();
            for e in a {
                arr.push(json_to_toml_value(e)?);
            }
            toml_edit::Value::Array(arr)
        }
        Value::Null | Value::Object(_) => return Err("unsupported TOML value in a server entry".to_owned()),
    })
}

fn table_eq(table: &toml_edit::Table, desired: &toml_edit::Table) -> bool {
    let norm = |t: &toml_edit::Table| {
        let mut c = t.clone();
        c.fmt();
        c.to_string()
    };
    norm(table) == norm(desired)
}

fn cli_name(client: AgentClient) -> Option<&'static str> {
    Some(match client {
        AgentClient::ClaudeCode => "claude",
        AgentClient::Codex => "codex",
        AgentClient::Gemini => "gemini",
        AgentClient::VsCode => "code",
        AgentClient::CopilotCli => "copilot",
        AgentClient::OpenHands => "openhands",
        AgentClient::OpenCode => "opencode",
        AgentClient::Cline | AgentClient::Continue => return None,
    })
}

fn config_dir_exists(client: AgentClient) -> bool {
    let Ok(home) = home() else { return false };
    let dir = match client {
        AgentClient::ClaudeCode => home.join(".claude"),
        AgentClient::Codex => home.join(".codex"),
        AgentClient::Gemini => home.join(".gemini"),
        AgentClient::OpenHands => home.join(".openhands"),
        AgentClient::CopilotCli => home.join(".copilot"),
        _ => return false,
    };
    dir.is_dir()
}

/// Whether `name` is an executable on `PATH`.
fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file() || candidate.with_extension("exe").is_file()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "russet-agentcfg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn claude_project_writes_mcp_json() {
        let dir = tmp();
        let reg = Registration::stdio(AgentClient::ClaudeCode, Scope::Project(dir.clone()), "russet");
        let changes = plan_install(&reg).unwrap();
        assert_eq!(changes.len(), 1);
        crate::apply(&changes).unwrap();
        let text = std::fs::read_to_string(dir.join(".mcp.json")).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["mcpServers"]["russet"]["command"], "russet");
        assert_eq!(v["mcpServers"]["russet"]["args"][3], "claude-code");
        // Idempotent.
        assert!(plan_install(&reg).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn codex_writes_toml_with_timeouts() {
        let dir = tmp();
        let reg = Registration::stdio(AgentClient::Codex, Scope::Project(dir.clone()), "russet");
        let changes = plan_install(&reg).unwrap();
        crate::apply(&changes).unwrap();
        let text = std::fs::read_to_string(dir.join(".codex").join("config.toml")).unwrap();
        let doc = text.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(doc["mcp_servers"]["russet"]["tool_timeout_sec"].as_integer(), Some(300));
        assert_eq!(doc["mcp_servers"]["russet"]["startup_timeout_sec"].as_integer(), Some(20));
        assert_eq!(doc["mcp_servers"]["russet"]["command"].as_str(), Some("russet"));
        assert!(plan_install(&reg).unwrap().is_empty(), "idempotent");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gemini_writes_timeout_and_trust() {
        let dir = tmp();
        let reg = Registration::stdio(AgentClient::Gemini, Scope::Project(dir.clone()), "russet");
        crate::apply(&plan_install(&reg).unwrap()).unwrap();
        let text = std::fs::read_to_string(dir.join(".gemini").join("settings.json")).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["mcpServers"]["russet"]["timeout"], 600000);
        assert_eq!(v["mcpServers"]["russet"]["trust"], false);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn vscode_project_writes_servers_stdio() {
        let dir = tmp();
        let reg = Registration::stdio(AgentClient::VsCode, Scope::Project(dir.clone()), "russet");
        crate::apply(&plan_install(&reg).unwrap()).unwrap();
        let text = std::fs::read_to_string(dir.join(".vscode").join("mcp.json")).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["servers"]["russet"]["type"], "stdio");
        assert_eq!(v["servers"]["russet"]["command"], "russet");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn vscode_user_scope_is_rejected() {
        let reg = Registration::stdio(AgentClient::VsCode, Scope::User, "russet");
        assert!(matches!(plan_install(&reg), Err(PlanError::ScopeUnsupported { .. })));
    }

    #[test]
    fn uninstall_then_reinstall_round_trips() {
        let dir = tmp();
        let reg = Registration::stdio(AgentClient::ClaudeCode, Scope::Project(dir.clone()), "russet");
        crate::apply(&plan_install(&reg).unwrap()).unwrap();
        crate::apply(&plan_uninstall(&reg).unwrap()).unwrap();
        let text = std::fs::read_to_string(dir.join(".mcp.json")).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(v["mcpServers"].get("russet").is_none());
        assert!(plan_uninstall(&reg).unwrap().is_empty(), "idempotent uninstall");
        // A backup was taken.
        assert!(edit::backup_path(&dir.join(".mcp.json")).exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
