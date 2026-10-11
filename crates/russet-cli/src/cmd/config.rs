// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet config get|set|unset|list|path|edit|schema [--scope …]` (ARCHITECTURE §8.7, §14). Owner: W3.
//!
//! Values travel as typed by the user; the daemon converts them to the key's type (`"2"` → `2`) and validates
//! the whole file before it writes anything. Without `--scope`, `set` and `unset` edit the file that owns the
//! key: `config.toml` for the global sections of §14.3, `russet.toml` otherwise.
//!
//! This module also holds the helpers the other W3 commands share ([`project_param`], [`report`]).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::config as p;
use serde::Serialize;
use serde_json::Value;

use crate::CliCtx;
use crate::global::GlobalArgs;

/// The sections of the global `config.toml` (ARCHITECTURE §14.3); every other section is a project section.
pub(crate) const GLOBAL_SECTIONS: &[&str] =
    &["daemon", "device", "storage", "devices", "web", "mcp", "fallback", "update", "privacy"];

/// The `config` command.
pub fn command() -> Command {
    let scope_read = || {
        Arg::new("scope")
            .long("scope")
            .value_name("SCOPE")
            .value_parser(["effective", "global", "project", "local"])
            .help("Which view to read (default: effective)")
    };
    let scope_write = || {
        Arg::new("scope")
            .long("scope")
            .value_name("SCOPE")
            .value_parser(["global", "project", "local"])
            .help("Which file to edit (default: the file that owns the key)")
    };
    Command::new("config")
        .about("Configuration: get, set, unset, list, path, edit [--scope …]")
        .subcommand_required(true)
        .subcommand(
            Command::new("get")
                .about("Print a key's value and where it comes from")
                .arg(Arg::new("key").value_name("KEY").help("A dotted key, for example daemon.idle-timeout"))
                .arg(scope_read()),
        )
        .subcommand(
            Command::new("set")
                .about("Set a key in a configuration file (comments and order are kept)")
                .arg(Arg::new("key").value_name("KEY").required(true))
                .arg(Arg::new("value").value_name("VALUE").required(true).allow_hyphen_values(true))
                .arg(scope_write()),
        )
        .subcommand(
            Command::new("unset")
                .about("Remove a key from a configuration file")
                .arg(Arg::new("key").value_name("KEY").required(true))
                .arg(scope_write()),
        )
        .subcommand(Command::new("list").about("Every key with its value").arg(scope_read()))
        .subcommand(Command::new("path").about("The configuration files and whether they exist").arg(scope_read()))
        .subcommand(
            Command::new("edit")
                .about("Open a configuration file in $VISUAL or $EDITOR, then validate it")
                .arg(scope_write().default_value("global")),
        )
        .subcommand(
            Command::new("schema").about("The JSON Schema of a configuration file").arg(
                Arg::new("file")
                    .long("file")
                    .value_name("FILE")
                    .value_parser(["project", "global", "accounts", "mcp-policy", "provision-policy"])
                    .default_value("project"),
            ),
        )
}

/// The project root for RPC params: `--project`, else the nearest directory at or above the current directory
/// that holds `russet.toml`. Paths on the wire are absolute (ARCHITECTURE §7.3).
pub(crate) fn project_param(global: &GlobalArgs) -> Result<Option<String>> {
    let cwd = std::env::current_dir().map_err(|e| {
        Error::new(codes::core::IO, format!("cannot read the current directory: {e}")).with_hint("cd somewhere else.")
    })?;
    let root = match &global.project {
        Some(p) => Some(absolute(&cwd, p)),
        None => cwd.ancestors().find(|d| d.join("russet.toml").is_file()).map(Path::to_path_buf),
    };
    Ok(root.map(|r| r.to_string_lossy().into_owned()))
}

fn absolute(cwd: &Path, p: &Path) -> PathBuf {
    let joined = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Hands a result to the reporter (one JSON document with `--json`).
pub(crate) fn report<T: Serialize>(cx: &CliCtx, v: &T) -> Result<()> {
    let value = serde_json::to_value(v).map_err(|e| {
        Error::new(codes::core::INTERNAL, format!("cannot encode the result: {e}")).with_hint("Report this bug.")
    })?;
    cx.reporter.result(&value);
    Ok(())
}

fn read_scope(s: Option<&String>) -> p::ConfigReadScope {
    match s.map(String::as_str) {
        Some("global") => p::ConfigReadScope::Global,
        Some("project") => p::ConfigReadScope::Project,
        Some("local") => p::ConfigReadScope::Local,
        _ => p::ConfigReadScope::Effective,
    }
}

fn write_scope(s: Option<&String>, key: &str) -> p::ConfigWriteScope {
    match s.map(String::as_str) {
        Some("global") => p::ConfigWriteScope::Global,
        Some("project") => p::ConfigWriteScope::Project,
        Some("local") => p::ConfigWriteScope::Local,
        _ => {
            let section = key.split('.').next().unwrap_or("").trim_matches('"');
            if GLOBAL_SECTIONS.contains(&section) { p::ConfigWriteScope::Global } else { p::ConfigWriteScope::Project }
        }
    }
}

/// One `russet config` invocation, as RPC params.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConfigRequest {
    Get(p::ConfigGetParams),
    Set(p::ConfigSetParams),
    Unset(p::ConfigUnsetParams),
    List(p::ConfigGetParams),
    Path(p::ConfigGetParams),
    Edit(p::ConfigGetParams),
    Schema(p::ConfigSchemaParams),
}

/// Reads the invocation from the root matches.
pub(crate) fn request(m: &ArgMatches, project: Option<String>) -> Result<ConfigRequest> {
    let Some(("config", m)) = m.subcommand() else {
        return Err(
            Error::new(codes::core::INVALID_ARGUMENT, "not a config command").with_hint("Run `russet config --help`.")
        );
    };
    let (name, a) = m.subcommand().ok_or_else(|| {
        Error::new(codes::core::INVALID_ARGUMENT, "no config subcommand").with_hint("Run `russet config --help`.")
    })?;
    let key = a.try_get_one::<String>("key").ok().flatten().cloned();
    let scope = a.try_get_one::<String>("scope").ok().flatten();
    Ok(match name {
        "get" => ConfigRequest::Get(p::ConfigGetParams { scope: read_scope(scope), project, key }),
        "set" => {
            let key = key.unwrap_or_default();
            let value = a.get_one::<String>("value").cloned().map(Value::String);
            ConfigRequest::Set(p::ConfigSetParams { scope: write_scope(scope, &key), project, key, value })
        }
        "unset" => {
            let key = key.unwrap_or_default();
            ConfigRequest::Unset(p::ConfigUnsetParams { scope: write_scope(scope, &key), project, key })
        }
        "list" => ConfigRequest::List(p::ConfigGetParams { scope: read_scope(scope), project, key: None }),
        "path" => ConfigRequest::Path(p::ConfigGetParams { scope: read_scope(scope), project, key: None }),
        "edit" => ConfigRequest::Edit(p::ConfigGetParams { scope: read_scope(scope), project, key: None }),
        "schema" => {
            let file = match a.get_one::<String>("file").map(String::as_str) {
                Some("global") => p::ConfigFileKind::Global,
                Some("accounts") => p::ConfigFileKind::Accounts,
                Some("mcp-policy") => p::ConfigFileKind::McpPolicy,
                Some("provision-policy") => p::ConfigFileKind::ProvisionPolicy,
                _ => p::ConfigFileKind::Project,
            };
            ConfigRequest::Schema(p::ConfigSchemaParams { file })
        }
        other => {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, format!("unknown config subcommand `{other}`"))
                .with_hint("Run `russet config --help`."));
        }
    })
}

/// Flattens a JSON document into dotted keys (`daemon.idle-timeout`), for `config list`.
pub(crate) fn flatten(v: &Value) -> serde_json::Map<String, Value> {
    fn walk(prefix: &str, v: &Value, out: &mut serde_json::Map<String, Value>) {
        match v {
            Value::Object(map) if !map.is_empty() => {
                for (k, child) in map {
                    let seg = if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                        k.clone()
                    } else {
                        format!("{k:?}")
                    };
                    let key = if prefix.is_empty() { seg } else { format!("{prefix}.{seg}") };
                    walk(&key, child, out);
                }
            }
            other => {
                if !prefix.is_empty() {
                    out.insert(prefix.to_owned(), other.clone());
                }
            }
        }
    }
    let mut out = serde_json::Map::new();
    walk("", v, &mut out);
    out
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let req = request(m, project_param(&cx.global)?)?;
    match req {
        ConfigRequest::Get(params) => report(cx, &cx.peer.call::<p::ConfigGet>(params).await?)?,
        ConfigRequest::Set(params) => report(cx, &cx.peer.call::<p::ConfigSet>(params).await?)?,
        ConfigRequest::Unset(params) => report(cx, &cx.peer.call::<p::ConfigUnset>(params).await?)?,
        ConfigRequest::Schema(params) => report(cx, &cx.peer.call::<p::ConfigSchema>(params).await?.schema)?,
        ConfigRequest::List(params) => {
            let r = cx.peer.call::<p::ConfigGet>(params).await?;
            report(cx, &flatten(&r.value.unwrap_or(Value::Null)))?;
        }
        ConfigRequest::Path(params) => {
            let r = cx.peer.call::<p::ConfigGet>(params).await?;
            report(cx, &serde_json::json!({ "files": r.files }))?;
        }
        ConfigRequest::Edit(params) => {
            let scope = params.scope;
            let r = cx.peer.call::<p::ConfigGet>(params.clone()).await?;
            let file = r.files.first().ok_or_else(|| {
                Error::new(codes::config::PROJECT_NOT_FOUND, "no file for this scope")
                    .with_hint("Run inside a project, or pass --project, for the project and local files.")
            })?;
            edit_file(Path::new(&file.path))?;
            // Validate what the user wrote; errors carry file:line:col.
            cx.peer.call::<p::ConfigGet>(p::ConfigGetParams { scope, ..params }).await?;
            report(cx, &serde_json::json!({ "path": file.path, "valid": true }))?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn edit_file(path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| if cfg!(windows) { "notepad".into() } else { "vi".into() });
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            Error::new(codes::core::IO, format!("cannot create {}: {e}", dir.display())).with_hint("Check permissions.")
        })?;
    }
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi");
    let status = std::process::Command::new(program).args(parts).arg(path).status().map_err(|e| {
        Error::new(codes::core::IO, format!("cannot start the editor `{editor}`: {e}"))
            .with_hint("Set VISUAL or EDITOR to your editor.")
    })?;
    if !status.success() {
        return Err(Error::new(codes::core::INTERNAL, format!("the editor exited with {status}"))
            .with_hint("The file was left as the editor saved it."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> ConfigRequest {
        let m = crate::root_command().try_get_matches_from(args).expect("parses");
        request(&m, Some("/p".into())).expect("request")
    }

    #[test]
    fn invocations_become_params() {
        let r = parse(&["russet", "config", "get", "daemon.idle-timeout"]);
        assert_eq!(
            r,
            ConfigRequest::Get(p::ConfigGetParams {
                scope: p::ConfigReadScope::Effective,
                project: Some("/p".into()),
                key: Some("daemon.idle-timeout".into())
            })
        );
        // §4.10: `russet config set --scope global fallback.pymobiledevice3 off`.
        let ConfigRequest::Set(set) =
            parse(&["russet", "config", "set", "--scope", "global", "fallback.pymobiledevice3", "off"])
        else {
            panic!("set")
        };
        assert_eq!((set.scope, set.value), (p::ConfigWriteScope::Global, Some(Value::from("off"))));
        // Without --scope the file that owns the key is edited.
        let ConfigRequest::Set(set) = parse(&["russet", "config", "set", "device.ddi.dir", "/ddi/iOS"]) else {
            panic!("set")
        };
        assert_eq!(set.scope, p::ConfigWriteScope::Global);
        let ConfigRequest::Set(set) = parse(&["russet", "config", "set", "run.device", "iphone12"]) else {
            panic!("set")
        };
        assert_eq!(set.scope, p::ConfigWriteScope::Project);
        let ConfigRequest::Set(set) = parse(&["russet", "config", "set", "build.swiftpm.swiftc-flags", "-Onone"])
        else {
            panic!("set")
        };
        assert_eq!(set.value, Some(Value::from("-Onone")), "hyphenated values are values");
        let ConfigRequest::Unset(u) = parse(&["russet", "config", "unset", "--scope", "local", "app.bundle-id"]) else {
            panic!("unset")
        };
        assert_eq!(u.scope, p::ConfigWriteScope::Local);
        assert!(matches!(
            parse(&["russet", "config", "schema", "--file", "mcp-policy"]),
            ConfigRequest::Schema(p::ConfigSchemaParams { file: p::ConfigFileKind::McpPolicy })
        ));
        assert!(matches!(parse(&["russet", "--json", "config", "path", "--scope", "local"]), ConfigRequest::Path(_)));
    }

    #[test]
    fn lists_flatten_to_dotted_keys() {
        let v = serde_json::json!({"daemon": {"idle-timeout": "30m"}, "signing": {"targets": {"PlugIns/W.appex": {"profile": "x"}}}, "app": {"resources": []}});
        let flat = flatten(&v);
        assert_eq!(flat["daemon.idle-timeout"], "30m");
        assert_eq!(flat["signing.targets.\"PlugIns/W.appex\".profile"], "x");
        assert_eq!(flat["app.resources"], serde_json::json!([]));
    }
}
