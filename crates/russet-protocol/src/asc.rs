// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `asc.*`: allowlisted App Store Connect GETs. Owner: W12.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.4 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod};

/// Params of `asc.get`: `{pathAndQuery, accountId?} → {status, body} (allowlisted GET; request-scoped token)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AscGetParams {
    /// An allowlisted `/v1/...` path and query (SIGN §11). Encoded dots, slashes and backslashes are refused.
    pub path_and_query: String,
    /// Omitted: the default account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub account_id: Option<String>,
}
/// Result of `asc.get`: the HTTP status and the parsed JSON body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AscGetResult {
    pub status: u16,
    pub body: serde_json::Value,
}
crate::rpc_method!(
    /// `asc.get` (class `read`). W12 (Wave 2).
    AscGet,
    "asc.get",
    AscGetParams => AscGetResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[AscGet::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
