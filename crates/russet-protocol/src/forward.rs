// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `forward.*`: port forwards (ARCHITECTURE §10.4). Owner: W4.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.2 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `forward.open`: `{udid?, target: {service}|{tunnelPort}|{usbmuxPort}, bind?: {tcp}|{unix}} → {leaseId, local}`.
pub type ForwardOpenParams = Unspecified;
/// Result of `forward.open`.
pub type ForwardOpenResult = Unspecified;
crate::rpc_method!(
    /// `forward.open` (class `device`). W4 (Wave 1).
    ForwardOpen,
    "forward.open",
    ForwardOpenParams => ForwardOpenResult,
    Permission::Device
);

/// Params of `forward.close`: `{leaseId} → {}`.
pub type ForwardCloseParams = Unspecified;
/// Result of `forward.close`.
pub type ForwardCloseResult = Unspecified;
crate::rpc_method!(
    /// `forward.close` (class `device`). W4 (Wave 1).
    ForwardClose,
    "forward.close",
    ForwardCloseParams => ForwardCloseResult,
    Permission::Device
);

/// Params of `forward.list`: `{} → {forwards[]}`.
pub type ForwardListParams = Unspecified;
/// Result of `forward.list`.
pub type ForwardListResult = Unspecified;
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
