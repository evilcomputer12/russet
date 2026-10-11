// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `subscription.*`: cancelling any subscription (ARCHITECTURE §6.4, §8.1). Owner: W2.
//!
//! Every `*.subscribe` and `*.watch` method returns a `subscriptionId`; its notifications carry it. A
//! subscription ends with `subscription.cancel` or when its connection closes.

use russet_core::ids::SubscriptionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod};

/// Params of `subscription.cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionCancelParams {
    pub subscription_id: SubscriptionId,
}

/// Result of `subscription.cancel`: `{}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SubscriptionCancelResult {}

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
