// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `forward.*`: port forwards (ARCHITECTURE §8.2, §10.4). Owner: W4.
//!
//! `target` is `{service}`, `{tunnelPort}` or `{usbmuxPort}` and `bind` is `{tcp}` or `{unix}`, exactly as
//! ARCHITECTURE §8.2 writes them: objects with exactly one field set (validated by the daemon), not tagged enums.

use russet_core::ids::LeaseId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod};

/// What a forward connects to on the device: exactly one field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardTargetParam {
    /// An RSD service, for example `com.apple.internal.dt.remote.debugproxy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub service: Option<String>,
    /// A TCP port on the device's tunnel address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tunnel_port: Option<u16>,
    /// A device port through a classic usbmux `Connect` (iproxy-style).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub usbmux_port: Option<u16>,
}

/// Where the forward listens: at most one field; default a 0600 Unix socket `runtime/fwd-<lease>.sock` on Linux
/// and `127.0.0.1:0` on Windows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardBindParam {
    /// A loopback `host:port` (`127.0.0.1:0` picks a port).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tcp: Option<String>,
    /// A Unix socket path (Linux).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unix: Option<String>,
}

/// Params of `forward.open`: `{udid?, target: {service}|{tunnelPort}|{usbmuxPort}, bind?: {tcp}|{unix}} → {leaseId, local}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardOpenParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    pub target: ForwardTargetParam,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bind: Option<ForwardBindParam>,
}

/// Result of `forward.open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardOpenResult {
    pub lease_id: LeaseId,
    /// `127.0.0.1:41250` or a socket path.
    pub local: String,
    /// The same as a gdb-remote URL for LLDB: `connect://…` or `unix-connect://…`.
    pub url: String,
}
crate::rpc_method!(
    /// `forward.open` (class `device`). W4 (Wave 1).
    ForwardOpen,
    "forward.open",
    ForwardOpenParams => ForwardOpenResult,
    Permission::Device
);

/// Params of `forward.close`: `{leaseId} → {}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardCloseParams {
    pub lease_id: LeaseId,
}

/// Result of `forward.close`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardCloseResult {}
crate::rpc_method!(
    /// `forward.close` (class `device`). W4 (Wave 1).
    ForwardClose,
    "forward.close",
    ForwardCloseParams => ForwardCloseResult,
    Permission::Device
);

/// Params of `forward.list`: `{} → {forwards[]}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardListParams {}

/// One open forward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardInfo {
    pub lease_id: LeaseId,
    pub udid: String,
    pub target: ForwardTargetParam,
    pub local: String,
    /// Connections accepted so far.
    #[ts(type = "number")]
    pub connections: u64,
    /// RFC 3339 UTC.
    pub opened_at: String,
}

/// Result of `forward.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardListResult {
    pub forwards: Vec<ForwardInfo>,
}
crate::rpc_method!(
    /// `forward.list` (class `read`). W4 (Wave 1).
    ForwardList,
    "forward.list",
    ForwardListParams => ForwardListResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[ForwardOpen::INFO, ForwardClose::INFO, ForwardList::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
