// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `secrets.*`: the secret store (ARCHITECTURE §8.1, §12.2). Owner: W3.
//!
//! Metadata only: no method returns a secret value. `secrets.set` reads a local file named by the client and
//! seals its content; it is accepted from `cli` and `ide` connections only, and a non-interactive CLI may run it
//! with `--yes` (one of the three local-file imports of §7.5) [D 85].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod};

/// The secret-store backend (`RUSSET_SECRETS_BACKEND`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SecretBackend {
    /// The master key in the OS keyring (Secret Service, Credential Manager).
    Keyring,
    /// The master key derived from `RUSSET_SECRETS_PASSPHRASE`.
    Passphrase,
    /// Read-only CI variables.
    Env,
    /// Tests.
    Memory,
}

/// What a secret is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SecretKind {
    AscKey,
    SigningKey,
    AppleIdSession,
    AppleIdPassword,
    AnisetteState,
    GithubToken,
    McpHttpToken,
    TcpToken,
}

/// Metadata of one secret; never its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SecretMeta {
    /// `<kind>/<name>`, for example `asc-key/2X9R4HXF34`.
    pub id: String,
    pub kind: SecretKind,
    pub name: String,
    pub label: String,
    pub backend: SecretBackend,
    /// RFC 3339 UTC.
    pub created_at: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
}

/// Params of `secrets.status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SecretsStatusParams {}

/// Result of `secrets.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SecretsStatusResult {
    pub backend: SecretBackend,
    pub available: bool,
    pub locked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
    /// The keyring entry holding the master key (`master-key` or `master-key-<sha256(RUSSET_HOME)[..8]>`), so a
    /// removed worktree's entry can be deleted (PLAN §14) [D 83].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub keyring_entry: Option<String>,
}

crate::rpc_method!(
    /// `secrets.status` (class `read`). W3 (Wave 1).
    SecretsStatus,
    "secrets.status",
    SecretsStatusParams => SecretsStatusResult,
    Permission::Read
);

/// Params of `secrets.list`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SecretsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<SecretKind>,
}

/// Result of `secrets.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SecretsListResult {
    pub items: Vec<SecretMeta>,
}

crate::rpc_method!(
    /// `secrets.list` (class `read`). W3 (Wave 1).
    SecretsList,
    "secrets.list",
    SecretsListParams => SecretsListResult,
    Permission::Read
);

/// Params of `secrets.set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SecretsSetParams {
    /// `<kind>/<name>`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
    /// The absolute path of a local file whose content becomes the secret; the value never travels over IPC.
    pub from_file: String,
}

/// Result of `secrets.set`: the stored secret's metadata (never the value).
pub type SecretsSetResult = SecretMeta;

crate::rpc_method!(
    /// `secrets.set` (class `secrets`). W3 (Wave 1).
    SecretsSet,
    "secrets.set",
    SecretsSetParams => SecretsSetResult,
    Permission::Secrets
);

/// Params of `secrets.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SecretsDeleteParams {
    /// `<kind>/<name>`.
    pub id: String,
}

/// Result of `secrets.delete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SecretsDeleteResult {
    pub deleted: bool,
}

crate::rpc_method!(
    /// `secrets.delete` (class `secrets`). W3 (Wave 1).
    SecretsDelete,
    "secrets.delete",
    SecretsDeleteParams => SecretsDeleteResult,
    Permission::Secrets
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[SecretsStatus::INFO, SecretsList::INFO, SecretsSet::INFO, SecretsDelete::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
