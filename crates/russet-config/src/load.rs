// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Reading and validating configuration files. Every problem is reported with `path:line:col` (ARCHITECTURE
//! §14.1): unknown keys as `config.unknownKey` (1302), everything else as `config.invalid` (1300).

use std::ops::Range;
use std::path::{Path, PathBuf};

use russet_core::error::{Error, Result, codes};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::keys;

/// The only file-format version (`schema = 1`).
pub const SCHEMA_VERSION: u32 = 1;

/// The name of the committed project file.
pub const PROJECT_FILE: &str = "russet.toml";
/// The name of the per-user, gitignored project file.
pub const LOCAL_FILE: &str = "russet.local.toml";
/// The global configuration file in the config directory.
pub const GLOBAL_FILE: &str = "config.toml";
/// The account list in the config directory.
pub const ACCOUNTS_FILE: &str = "accounts.toml";
/// The MCP policy in the config directory.
pub const MCP_POLICY_FILE: &str = "mcp-policy.toml";
/// The provisioning policy in the config directory.
pub const PROVISION_POLICY_FILE: &str = "provision-policy.toml";

/// The configuration files Russet reads (`config.schema {file}` names the schema of each kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigFile {
    /// `russet.toml`; `russet.local.toml` uses the same schema.
    Project,
    /// `config.toml`.
    Global,
    /// `accounts.toml`.
    Accounts,
    /// `mcp-policy.toml`.
    McpPolicy,
    /// `provision-policy.toml`.
    ProvisionPolicy,
}

impl ConfigFile {
    /// Every kind, in documentation order.
    pub const ALL: [ConfigFile; 5] = [
        ConfigFile::Project,
        ConfigFile::Global,
        ConfigFile::Accounts,
        ConfigFile::McpPolicy,
        ConfigFile::ProvisionPolicy,
    ];

    /// The wire and schema-file name: `project`, `global`, `accounts`, `mcp-policy`, `provision-policy`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
            Self::Accounts => "accounts",
            Self::McpPolicy => "mcp-policy",
            Self::ProvisionPolicy => "provision-policy",
        }
    }

    /// The file name on disk (`russet.toml` for the project kind).
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Project => PROJECT_FILE,
            Self::Global => GLOBAL_FILE,
            Self::Accounts => ACCOUNTS_FILE,
            Self::McpPolicy => MCP_POLICY_FILE,
            Self::ProvisionPolicy => PROVISION_POLICY_FILE,
        }
    }
}

/// Where a configuration value came from (ARCHITECTURE §14.1, highest precedence first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SourceScope {
    /// A `RUSSET_*` environment variable.
    Env,
    /// `russet.local.toml`.
    Local,
    /// `russet.toml`.
    Project,
    /// `config.toml`, `mcp-policy.toml` and the other files of the config directory.
    Global,
    /// The built-in default.
    Default,
}

/// One source of configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceInfo {
    pub scope: SourceScope,
    /// The file, for file scopes; the variable name is in `var` for [`SourceScope::Env`].
    pub path: Option<PathBuf>,
    /// The environment variable, for [`SourceScope::Env`].
    pub var: Option<String>,
}

impl SourceInfo {
    /// A file source.
    pub fn file(scope: SourceScope, path: impl Into<PathBuf>) -> Self {
        Self { scope, path: Some(path.into()), var: None }
    }

    /// An environment-variable source.
    pub fn env(var: impl Into<String>) -> Self {
        Self { scope: SourceScope::Env, path: None, var: Some(var.into()) }
    }

    /// The built-in defaults.
    pub fn default_values() -> Self {
        Self { scope: SourceScope::Default, path: None, var: None }
    }
}

/// A parsed file: the typed value, the raw table (for layering and `config get`) and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded<T> {
    pub value: T,
    pub raw: toml::Table,
    pub source: SourceInfo,
}

/// A file's identity for cache invalidation: modification time and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub modified: Option<std::time::SystemTime>,
    pub len: u64,
}

impl Stamp {
    /// The stamp of `path`, or `None` when it does not exist.
    pub fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        Some(Self { modified: m.modified().ok(), len: m.len() })
    }
}

/// 1-based line and column (in characters) of byte offset `at` in `text`.
pub fn line_col(text: &str, at: usize) -> (usize, usize) {
    let at = at.min(text.len());
    let before = text.get(..at).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let col = before.get(line_start..).map_or(0, |s| s.chars().count()) + 1;
    (line, col)
}

/// A `config.invalid` or `config.unknownKey` error located at `span` in `path`.
pub fn located_error(path: &Path, text: &str, span: Option<Range<usize>>, message: &str, unknown_key: bool) -> Error {
    let (code, hint) = if unknown_key {
        (codes::config::UNKNOWN_KEY, "Remove or rename the key; `russet config schema` lists the valid keys.")
    } else {
        (codes::config::INVALID, "Fix the value at that position; `russet doctor` re-checks every configuration file.")
    };
    let message = message.trim();
    let (line, column, located) = match span {
        Some(s) => {
            let (l, c) = line_col(text, s.start);
            (l, c, format!("{}:{l}:{c}: {message}", path.display()))
        }
        None => (0, 0, format!("{}: {message}", path.display())),
    };
    let mut details = serde_json::json!({ "path": path });
    if line > 0 {
        details["line"] = line.into();
        details["column"] = column.into();
    }
    Error::new(code, located).with_hint(hint).with_details(details)
}

fn is_unknown_key_message(message: &str) -> bool {
    message.trim_start().starts_with("unknown field")
}

/// Parses `text` (the content of `path`) as `T`, checking `schema = 1`.
pub fn parse<T: DeserializeOwned>(path: &Path, text: &str) -> Result<(T, toml::Table)> {
    let raw: toml::Table = toml::from_str(text)
        .map_err(|e| located_error(path, text, e.span(), e.message(), is_unknown_key_message(e.message())))?;
    if let Some(v) = raw.get("schema") {
        let ok = v.as_integer().is_some_and(|n| n == i64::from(SCHEMA_VERSION));
        if !ok {
            let span = keys::span_of(text, &["schema"]);
            return Err(located_error(
                path,
                text,
                span,
                &format!("unsupported schema `{v}`; this version of Russet reads schema = {SCHEMA_VERSION}"),
                false,
            ));
        }
    }
    let value: T = toml::from_str(text)
        .map_err(|e| located_error(path, text, e.span(), e.message(), is_unknown_key_message(e.message())))?;
    Ok((value, raw))
}

/// Reads and parses `path`; `Ok(None)` when it does not exist.
pub fn read<T: DeserializeOwned>(path: &Path, scope: SourceScope) -> Result<Option<Loaded<T>>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Error::new(codes::core::IO, format!("cannot read {}: {e}", path.display()))
                .with_hint("Check the file's permissions.")
                .caused_by(e));
        }
    };
    let (value, raw) = parse::<T>(path, &text)?;
    Ok(Some(Loaded { value, raw, source: SourceInfo::file(scope, path) }))
}

/// Walks up from `start` to the nearest directory that holds `russet.toml`.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|d| d.join(PROJECT_FILE).is_file()).map(Path::to_path_buf)
}

/// Deep-merges `over` into `base`: tables merge key by key, everything else (arrays included) is replaced.
pub fn merge_tables(base: &mut toml::Table, over: &toml::Table) {
    for (k, v) in over {
        match (base.get_mut(k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge_tables(b, o),
            _ => {
                base.insert(k.clone(), v.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global::GlobalConfig;
    use crate::project::ProjectConfig;

    #[test]
    fn line_and_column_are_one_based() {
        let text = "a = 1\nbb = \"x\"\n";
        assert_eq!(line_col(text, 0), (1, 1));
        assert_eq!(line_col(text, 6), (2, 1));
        assert_eq!(line_col(text, 11), (2, 6));
        assert_eq!(line_col("é = 1", 5), (1, 5), "columns count characters, not bytes");
    }

    #[test]
    fn unknown_keys_are_located() {
        let text = "schema = 1\n[daemon]\nidle-timeout = \"5m\"\nidle-timeouts = \"5m\"\n";
        let err = parse::<GlobalConfig>(Path::new("/c/config.toml"), text).expect_err("typo");
        assert_eq!(err.code(), codes::config::UNKNOWN_KEY);
        assert!(err.message().starts_with("/c/config.toml:4:1: unknown field `idle-timeouts`"), "{}", err.message());
        let d = err.to_data().details.expect("details");
        assert_eq!((d["line"].as_u64(), d["column"].as_u64()), (Some(4), Some(1)));
    }

    #[test]
    fn invalid_values_are_located() {
        let text = "[daemon]\nmax-build-jobs = 1\nidle-timeout = \"thirty\"\n";
        let err = parse::<GlobalConfig>(Path::new("/c/config.toml"), text).expect_err("bad duration");
        assert_eq!(err.code(), codes::config::INVALID);
        assert!(err.message().starts_with("/c/config.toml:3:16:"), "{}", err.message());
        let text = "[run]\ninstall = \"sometimes\"\n";
        let err = parse::<ProjectConfig>(Path::new("/p/russet.toml"), text).expect_err("bad enum");
        assert_eq!(err.code(), codes::config::INVALID);
        assert!(err.message().starts_with("/p/russet.toml:2:11:"), "{}", err.message());
        let err = parse::<ProjectConfig>(Path::new("/p/russet.toml"), "schema = 2\n").expect_err("schema 2");
        assert!(err.message().starts_with("/p/russet.toml:1:10: unsupported schema"), "{}", err.message());
        let err = parse::<ProjectConfig>(Path::new("/p/russet.toml"), "[app\n").expect_err("syntax");
        assert_eq!(err.code(), codes::config::INVALID);
        assert!(err.message().starts_with("/p/russet.toml:1:"), "{}", err.message());
    }

    #[test]
    fn merging_replaces_arrays_and_merges_tables() {
        let mut base: toml::Table = toml::from_str("[run]\nargs = [\"a\"]\ndevice = \"auto\"\n").expect("base");
        let over: toml::Table = toml::from_str("[run]\nargs = [\"b\"]\n").expect("over");
        merge_tables(&mut base, &over);
        let run = base.get("run").and_then(toml::Value::as_table).expect("run");
        assert_eq!(run.get("args"), Some(&toml::Value::Array(vec!["b".into()])));
        assert_eq!(run.get("device").and_then(toml::Value::as_str), Some("auto"));
    }
}
