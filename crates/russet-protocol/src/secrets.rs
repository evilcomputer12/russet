// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `secrets.*`: the secret store (metadata only; values never cross IPC). Owner: W3.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.1 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `secrets.status`: `{} → {backend, available, locked, reason?}`.
pub type SecretsStatusParams = Unspecified;
/// Result of `secrets.status`.
pub type SecretsStatusResult = Unspecified;
crate::rpc_method!(
    /// `secrets.status` (class `read`). W3 (Wave 1).
    SecretsStatus,
    "secrets.status",
    SecretsStatusParams => SecretsStatusResult,
    Permission::Read
);

/// Params of `secrets.list`: `{kind?} → {items: SecretMeta[]} (metadata only)`.
pub type SecretsListParams = Unspecified;
/// Result of `secrets.list`.
pub type SecretsListResult = Unspecified;
crate::rpc_method!(
    /// `secrets.list` (class `read`). W3 (Wave 1).
    SecretsList,
    "secrets.list",
    SecretsListParams => SecretsListResult,
    Permission::Read
);

/// Params of `secrets.set`: `{id, label?, fromFile} → SecretMeta (cli/ide only; never echoes)`.
pub type SecretsSetParams = Unspecified;
/// Result of `secrets.set`.
pub type SecretsSetResult = Unspecified;
crate::rpc_method!(
    /// `secrets.set` (class `secrets`). W3 (Wave 1).
    SecretsSet,
    "secrets.set",
    SecretsSetParams => SecretsSetResult,
    Permission::Secrets
);

/// Params of `secrets.delete`: `{id} → {deleted}`.
pub type SecretsDeleteParams = Unspecified;
/// Result of `secrets.delete`.
pub type SecretsDeleteResult = Unspecified;
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
