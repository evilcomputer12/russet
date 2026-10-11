// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `accounts.toml` (ARCHITECTURE §14.4): the account list, written by the daemon's `account.*` methods
//! (W6, W12, W22) and kept human-readable. This module owns the file format and its schema; it holds no
//! secrets, only `secret:<kind>/<name>` references into the secret store.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The account file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[schemars(title = "accounts.toml", description = "Russet's accounts (ARCHITECTURE §14.4); no secrets.")]
pub struct AccountsFile {
    /// The file-format version; only `1` exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    /// The default account id, for example `asc:<KEYID>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(rename = "account")]
    pub accounts: Vec<AccountEntry>,
}

/// One `[[account]]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AccountEntry {
    /// `asc:<KEYID>` or `appleid:<DSID>`.
    pub id: String,
    pub kind: AccountKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_id: Option<String>,
    /// Discovered from a certificate OU, else a bundle-ID seed id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_tier: Option<TeamTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apple_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anisette: Option<Anisette>,
    /// A reference into the secret store: `secret:<kind>/<name>`.
    pub secret: String,
    /// Optional provenance, for example `xtool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
}

/// `[[account]] kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AccountKind {
    AscTeamKey,
    AscIndividualKey,
    AppleId,
}

/// `[[account]] team-tier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TeamTier {
    Free,
    Paid,
}

/// `[[account]] anisette`: `"local"` or `{ remote = "https://…" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Anisette {
    Named(AnisetteKind),
    Remote(RemoteAnisette),
}

/// The named anisette providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AnisetteKind {
    Local,
}

/// `{ remote = "https://…" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RemoteAnisette {
    pub remote: String,
}
