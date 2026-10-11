// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `identity.*`: signing identities (ARCHITECTURE §8.4; SIGN §6.3). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! The `importP12`/`exportP12` rows keep placeholder DTOs ([`Unspecified`]) until W12 defines them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// A certificate kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CertificateKind {
    Development,
    Distribution,
}

/// A signing identity's public part (`SigningIdentity`). The private key never leaves the daemon, and nothing here
/// names the team unmasked or the certificate's subject (it names a person).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SigningIdentity {
    /// The certificate serial (uppercase hex without leading zeros), what `sign.run {identityId}` takes; the
    /// daemon also accepts the SHA-1 fingerprint. The daemon's internal `<team>/<serial>` id never goes on the wire.
    pub id: String,
    /// Masked (`*******XYZ`).
    pub team_id: String,
    pub kind: CertificateKind,
    /// Uppercase hex without leading zeros (the portal's form).
    pub serial: String,
    pub label: String,
    /// RFC 3339.
    pub not_after: String,
    pub expired: bool,
    /// Lower-case hex SHA-1 of the certificate.
    pub sha1: String,
    /// `xtool` for the one-time import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub imported_from: Option<String>,
}

/// Params of `identity.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityListParams {
    /// Only this team's identities: a team ID, or its masked form as the wire shows it (`*******345`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub team: Option<String>,
}
/// Result of `identity.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityListResult {
    pub identities: Vec<SigningIdentity>,
}
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
