// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `provision.*`: provisioning plans (GET-only planning; approved apply) (ARCHITECTURE §8.4; SIGN §6.4). Owner:
//! W6 (Wave 1), W12 (Wave 2).
//!
//! In Wave 1, `provision.apply` performs zero-mutation plans (it downloads and caches the profiles the plan
//! reuses); a plan with account changes answers `perm.approvalRequired` without an approval, and
//! `core.notImplemented` with one (W12 applies them). `provision.capabilities` keeps a placeholder DTO.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::sign::Distribution;
use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `provision.plan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionPlanParams {
    /// The project root (absolute): its `russet.toml` and `russet.local.toml` give the bundle IDs.
    pub project: String,
    /// Default `development`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub distribution: Option<Distribution>,
    /// UDIDs the profiles must list (development and ad hoc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub devices: Option<Vec<String>>,
    /// Omitted: `[signing] account`, else the default account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub account_id: Option<String>,
}

/// The kind of an account change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum MutationKind {
    CreateCertificate,
    RegisterDevice,
    RegisterBundleId,
    EnableCapability,
    CreateProfile,
    DeleteProfile,
}

/// One account change a plan needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanMutation {
    pub kind: MutationKind,
    /// A one-line description (UDIDs redacted).
    pub summary: String,
    /// The bundle ID it concerns, when it concerns one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
}

/// An existing profile the plan reuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlannedProfile {
    /// `""` for the app.
    pub rel_path: String,
    pub bundle_id: String,
    pub profile_id: String,
    pub uuid: String,
    pub name: String,
    pub expires: String,
}

/// Result of `provision.plan` (`ProvisionPlan`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionPlanResult {
    /// `pln_…`; plans can be applied for 10 minutes (then `provision.planStale`).
    pub plan_id: String,
    /// Hex SHA-256 the approval binds to.
    pub digest: String,
    pub account_id: String,
    /// Masked.
    pub team_id: String,
    pub distribution: Distribution,
    /// The identity that signs, when one is usable: its `identity.list` id (the certificate serial).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub identity_id: Option<String>,
    /// Account changes; empty for a plan `provision.apply` performs without an approval.
    pub mutations: Vec<PlanMutation>,
    pub profiles: Vec<PlannedProfile>,
    pub warnings: Vec<String>,
}
crate::rpc_method!(
    /// `provision.plan` (class `read`). W6 (Wave 1).
    ProvisionPlan,
    "provision.plan",
    ProvisionPlanParams => ProvisionPlanResult,
    Permission::Read
);

/// Params of `provision.apply`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionApplyParams {
    pub plan_id: String,
    /// `apr_…`; a zero-mutation plan needs none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval: Option<String>,
}

/// One provisioned bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BundleProvision {
    pub rel_path: String,
    pub bundle_id: String,
    pub profile_uuid: String,
    /// The cached `.mobileprovision` `sign.run` uses.
    pub profile_path: String,
}

/// Result of `provision.apply` (`ProvisionOutcome`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionApplyResult {
    /// The identity that signs: its `identity.list` id (the certificate serial).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub identity_id: Option<String>,
    pub bundles: Vec<BundleProvision>,
}
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
