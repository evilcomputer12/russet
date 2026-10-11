// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `config.*`: configuration files (ARCHITECTURE §8.1, §14). Owner: W3.
//!
//! `config.get` reads the effective view (the precedence of §14.1, defaults filled in) or one file as written;
//! `config.set`/`config.unset` edit one file, keeping comments and order, and validate it against its schema
//! before saving. Their class is `project` for the project and local files and `system` for the global file,
//! resolved per call [D 66]. `config.changed` goes to every connection after an edit (W2 forwards it).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod, RpcNotification};

/// The view `config.get` reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigReadScope {
    /// The precedence of ARCHITECTURE §14.1, built-in defaults filled in.
    #[default]
    Effective,
    /// `config.toml` as written.
    Global,
    /// `russet.toml` as written.
    Project,
    /// `russet.local.toml` as written.
    Local,
}

/// The file `config.set`/`config.unset` edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigWriteScope {
    /// `config.toml` (class `system`).
    Global,
    /// `russet.toml` (class `project`).
    Project,
    /// `russet.local.toml` (class `project`).
    Local,
}

/// Where a value came from (ARCHITECTURE §14.1, highest precedence first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigSourceScope {
    Env,
    Local,
    Project,
    Global,
    Default,
}

/// One source of a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSource {
    pub scope: ConfigSourceScope,
    /// The file, for file scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    /// The environment variable, for `env`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub var: Option<String>,
}

/// A candidate file of the requested scope (`russet config path`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFileInfo {
    pub scope: ConfigWriteScope,
    pub path: String,
    pub exists: bool,
}

/// Params of `config.get`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigGetParams {
    #[serde(default)]
    pub scope: ConfigReadScope,
    /// The project root (absolute); needed for the project and local scopes and for project keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    /// A dotted key in TOML syntax (`daemon.idle-timeout`); omitted = the whole view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub key: Option<String>,
}

/// Result of `config.get`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigGetResult {
    /// The value; omitted when the requested file does not set the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub value: Option<serde_json::Value>,
    /// Where the value came from, highest precedence first.
    #[serde(default)]
    pub sources: Vec<ConfigSource>,
    /// The candidate files of the scope and whether they exist.
    #[serde(default)]
    pub files: Vec<ConfigFileInfo>,
}

crate::rpc_method!(
    /// `config.get` (class `read`). W3 (Wave 1).
    ConfigGet,
    "config.get",
    ConfigGetParams => ConfigGetResult,
    Permission::Read
);

/// Params of `config.set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSetParams {
    pub scope: ConfigWriteScope,
    /// The project root (absolute); needed for the project and local scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    /// A dotted key in TOML syntax.
    pub key: String,
    /// The new value; strings are converted to the key's type (`"2"` → `2`). Omitted or `null` unsets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub value: Option<serde_json::Value>,
}

/// Result of `config.set` and `config.unset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSetResult {
    /// The file that was (or would have been) written.
    pub path: String,
    /// Whether the file changed.
    pub changed: bool,
}

crate::rpc_method!(
    /// `config.set` (class `project` for the project and local scopes, `system` for global; resolved per call,
    /// ceiling `system`). W3 (Wave 1).
    ConfigSet,
    "config.set",
    ConfigSetParams => ConfigSetResult,
    Permission::System, permission_fn = config_set_permission
);

/// The class of a write to `scope`'s file [D 66].
pub fn write_permission(scope: ConfigWriteScope) -> Permission {
    match scope {
        ConfigWriteScope::Global => Permission::System,
        ConfigWriteScope::Project | ConfigWriteScope::Local => Permission::Project,
    }
}

/// Per-call class of `config.set` [D 66]: `project` for the project and local scopes, `system` for global.
pub fn config_set_permission(
    p: &ConfigSetParams,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(write_permission(p.scope))
}

/// Params of `config.unset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUnsetParams {
    pub scope: ConfigWriteScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    pub key: String,
}

/// Result of `config.unset`.
pub type ConfigUnsetResult = ConfigSetResult;

crate::rpc_method!(
    /// `config.unset` (class `project` for the project and local scopes, `system` for global; resolved per
    /// call, ceiling `system`). W3 (Wave 1).
    ConfigUnset,
    "config.unset",
    ConfigUnsetParams => ConfigUnsetResult,
    Permission::System, permission_fn = config_unset_permission
);

/// Per-call class of `config.unset` [D 66]: `project` for the project and local scopes, `system` for global.
pub fn config_unset_permission(
    p: &ConfigUnsetParams,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(write_permission(p.scope))
}

/// The configuration files with a schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigFileKind {
    /// `russet.toml` and `russet.local.toml`.
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

/// Params of `config.schema`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSchemaParams {
    pub file: ConfigFileKind,
}

/// Result of `config.schema`: the file's JSON Schema (draft 2020-12).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSchemaResult {
    pub schema: serde_json::Value,
}

crate::rpc_method!(
    /// `config.schema` (class `read`). W3 (Wave 1).
    ConfigSchema,
    "config.schema",
    ConfigSchemaParams => ConfigSchemaResult,
    Permission::Read
);

/// Params of the `config.changed` notification (to every connection).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChangedParams {
    pub scope: ConfigWriteScope,
    pub path: String,
}

crate::rpc_notification!(
    /// `config.changed` (daemon → client). W3.
    ConfigChanged,
    "config.changed",
    ConfigChangedParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[ConfigGet::INFO, ConfigSet::INFO, ConfigUnset::INFO, ConfigSchema::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[ConfigChanged::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
