// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `config.*`: configuration files (ARCHITECTURE §14). Owner: W3.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.1 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod, RpcNotification, Unspecified};

/// Params of `config.get`: `{scope: effective|global|project|local, project?, key?} → {value, sources[{scope,path}]}`.
pub type ConfigGetParams = Unspecified;
/// Result of `config.get`.
pub type ConfigGetResult = Unspecified;
crate::rpc_method!(
    /// `config.get` (class `read`). W3 (Wave 1).
    ConfigGet,
    "config.get",
    ConfigGetParams => ConfigGetResult,
    Permission::Read
);

/// Params of `config.set`: `{scope, project?, key, value?} → {path, changed} (class: project for project/local, system for global)`.
pub type ConfigSetParams = Unspecified;
/// Result of `config.set`.
pub type ConfigSetResult = Unspecified;
crate::rpc_method!(
    /// `config.set` (class `system`). W3 (Wave 1).
    ConfigSet,
    "config.set",
    ConfigSetParams => ConfigSetResult,
    Permission::System, permission_fn = config_set_permission
);

/// Per-call class of `config.set` [D 66]: `project` for the project and local scopes, `system` for global.
///
/// F0 stub: fails closed at the ceiling (`system`) until the owner reads the params.
pub fn config_set_permission(
    _p: &ConfigSetParams,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(Permission::System)
}

/// Params of `config.unset`: `{scope, project?, key} → {path, changed} (class: project for project/local, system for global)`.
pub type ConfigUnsetParams = Unspecified;
/// Result of `config.unset`.
pub type ConfigUnsetResult = Unspecified;
crate::rpc_method!(
    /// `config.unset` (class `system`). W3 (Wave 1).
    ConfigUnset,
    "config.unset",
    ConfigUnsetParams => ConfigUnsetResult,
    Permission::System, permission_fn = config_unset_permission
);

/// Per-call class of `config.unset` [D 66]: `project` for the project and local scopes, `system` for global.
///
/// F0 stub: fails closed at the ceiling (`system`) until the owner reads the params.
pub fn config_unset_permission(
    _p: &ConfigUnsetParams,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(Permission::System)
}

/// Params of `config.schema`: `{file: project|global|accounts|mcp-policy|provision-policy} → {schema}`.
pub type ConfigSchemaParams = Unspecified;
/// Result of `config.schema`.
pub type ConfigSchemaResult = Unspecified;
crate::rpc_method!(
    /// `config.schema` (class `read`). W3 (Wave 1).
    ConfigSchema,
    "config.schema",
    ConfigSchemaParams => ConfigSchemaResult,
    Permission::Read
);

/// Params of the `config.changed` notification: `{scope, path}; to all`.
pub type ConfigChangedParams = Unspecified;
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
