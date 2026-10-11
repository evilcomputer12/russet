// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `provision.*`: provisioning plans (GET-only planning; approved apply). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.4 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `provision.plan`: `{project, distribution?: development|ad-hoc|app-store-connect, devices?, accountId?} → ProvisionPlan`.
pub type ProvisionPlanParams = Unspecified;
/// Result of `provision.plan`.
pub type ProvisionPlanResult = Unspecified;
crate::rpc_method!(
    /// `provision.plan` (class `read`). W6 (Wave 1).
    ProvisionPlan,
    "provision.plan",
    ProvisionPlanParams => ProvisionPlanResult,
    Permission::Read
);

/// Params of `provision.apply`: `{planId, approval?} → ProvisionOutcome (a zero-mutation plan needs no approval)`.
pub type ProvisionApplyParams = Unspecified;
/// Result of `provision.apply`.
pub type ProvisionApplyResult = Unspecified;
crate::rpc_method!(
    /// `provision.apply` (class `account`, long-running). W6 (zero-mutation, Wave 1) / W12 (Wave 2).
    ProvisionApply,
    "provision.apply",
    ProvisionApplyParams => ProvisionApplyResult,
    Permission::Account, long_running
);

/// Params of `provision.capabilities`: `{accountId?} → {capabilities[{id, title, entitlements, freeTeam, needsPortalStep, docs}]}`.
pub type ProvisionCapabilitiesParams = Unspecified;
/// Result of `provision.capabilities`.
pub type ProvisionCapabilitiesResult = Unspecified;
crate::rpc_method!(
    /// `provision.capabilities` (class `read`). W12 (Wave 2).
    ProvisionCapabilities,
    "provision.capabilities",
    ProvisionCapabilitiesParams => ProvisionCapabilitiesResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[ProvisionPlan::INFO, ProvisionApply::INFO, ProvisionCapabilities::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
