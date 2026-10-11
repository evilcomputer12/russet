// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `subscription.*`: cancelling any subscription. Owner: W2.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.1 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `subscription.cancel`: `{subscriptionId} → {}`.
pub type SubscriptionCancelParams = Unspecified;
/// Result of `subscription.cancel`.
pub type SubscriptionCancelResult = Unspecified;
crate::rpc_method!(
    /// `subscription.cancel` (class `read`). W2 (Wave 1).
    SubscriptionCancel,
    "subscription.cancel",
    SubscriptionCancelParams => SubscriptionCancelResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[SubscriptionCancel::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
