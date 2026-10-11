// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `identity.*`: signing identities. Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.4 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `identity.list`: `{team?} → {identities: SigningIdentity[]}`.
pub type IdentityListParams = Unspecified;
/// Result of `identity.list`.
pub type IdentityListResult = Unspecified;
crate::rpc_method!(
    /// `identity.list` (class `read`). W6 (Wave 1).
    IdentityList,
    "identity.list",
    IdentityListParams => IdentityListResult,
    Permission::Read
);

/// Params of `identity.importP12`: `{path, passwordPrompt: true} → SigningIdentity`.
pub type IdentityImportP12Params = Unspecified;
/// Result of `identity.importP12`.
pub type IdentityImportP12Result = Unspecified;
crate::rpc_method!(
    /// `identity.importP12` (class `secrets`). W12 (Wave 2).
    IdentityImportP12,
    "identity.importP12",
    IdentityImportP12Params => IdentityImportP12Result,
    Permission::Secrets
);

/// Params of `identity.exportP12`: `{identityId, out} → {}`.
pub type IdentityExportP12Params = Unspecified;
/// Result of `identity.exportP12`.
pub type IdentityExportP12Result = Unspecified;
crate::rpc_method!(
    /// `identity.exportP12` (class `secrets`). W12 (Wave 2).
    IdentityExportP12,
    "identity.exportP12",
    IdentityExportP12Params => IdentityExportP12Result,
    Permission::Secrets
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[IdentityList::INFO, IdentityImportP12::INFO, IdentityExportP12::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
