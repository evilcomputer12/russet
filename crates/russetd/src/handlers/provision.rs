// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `provision` handlers (§8.4). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! `provision.plan` is GET-only; `provision.apply` performs zero-mutation plans and refuses account changes
//! without an approval (`perm.approvalRequired`); with one they wait for W12 (`core.notImplemented`).
//! `provision.capabilities` is W12's.

use russet_account::{AccountId, AccountService, ApprovalRef, Mutation, ProvisionOutcome, ProvisionPlan};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_ipc::Router;
use russet_protocol::provision as p;
use russet_protocol::sign::Distribution as WireDistribution;

use super::account::absolute;
use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ProvisionPlan, _, _>(|cx: DaemonCtx, _rq, params: p::ProvisionPlanParams| async move {
        plan(&cx.accounts, params).await
    });
    r.request::<p::ProvisionApply, _, _>(|cx: DaemonCtx, _rq, params: p::ProvisionApplyParams| async move {
        apply(&cx.accounts, params).await
    });
    r.request::<p::ProvisionCapabilities, _, _>(not_implemented::<p::ProvisionCapabilities>);
}

/// The domain distribution of a wire value.
pub fn distribution(d: WireDistribution) -> russet_sign::Distribution {
    match d {
        WireDistribution::Development => russet_sign::Distribution::Development,
        WireDistribution::AdHoc => russet_sign::Distribution::AdHoc,
        WireDistribution::AppStoreConnect => russet_sign::Distribution::AppStore,
    }
}

/// The wire value of a domain distribution.
pub fn wire_distribution(d: russet_sign::Distribution) -> WireDistribution {
    match d {
        russet_sign::Distribution::Development => WireDistribution::Development,
        russet_sign::Distribution::AdHoc => WireDistribution::AdHoc,
        russet_sign::Distribution::AppStore => WireDistribution::AppStoreConnect,
    }
}

/// `provision.plan`.
pub async fn plan(svc: &AccountService, params: p::ProvisionPlanParams) -> Result<p::ProvisionPlanResult> {
    let project = absolute(&params.project, "project")?;
    let d = distribution(params.distribution.unwrap_or_default());
    let account = params.account_id.map(AccountId);
    let plan = svc.plan_project(&project, d, params.devices.unwrap_or_default(), account.as_ref()).await?;
    Ok(plan_dto(&plan))
}

/// `provision.apply`.
pub async fn apply(svc: &AccountService, params: p::ProvisionApplyParams) -> Result<p::ProvisionApplyResult> {
    let approval_id = match params.approval {
        Some(a) => Some(
            serde_json::from_value(serde_json::Value::String(a))
                .map_err(|_| Error::new(codes::core::INVALID_ARGUMENT, "approval must be an approval id (`apr_…`)"))?,
        ),
        None => None,
    };
    let outcome = svc.apply_plan(&params.plan_id, &ApprovalRef { approval_id }).await?;
    Ok(outcome_dto(&outcome))
}

/// The wire form of a plan (team masked, UDIDs already redacted by the planner).
pub fn plan_dto(plan: &ProvisionPlan) -> p::ProvisionPlanResult {
    p::ProvisionPlanResult {
        plan_id: plan.plan_id.0.clone(),
        digest: russet_sign::hex(&plan.digest),
        account_id: plan.account_id.0.clone(),
        team_id: russet_sign::mask_team(&plan.team_id),
        distribution: wire_distribution(plan.distribution),
        identity_id: plan.identity.as_ref().map(|i| i.0.clone()),
        mutations: plan.mutations.iter().map(mutation_dto).collect(),
        profiles: plan
            .profiles
            .iter()
            .map(|x| p::PlannedProfile {
                rel_path: x.rel_path.clone(),
                bundle_id: x.bundle_id.clone(),
                profile_id: x.profile_id.clone(),
                uuid: x.uuid.clone(),
                name: x.name.clone(),
                expires: x.expires.clone(),
            })
            .collect(),
        warnings: plan.warnings.iter().map(|w| w.message.clone()).collect(),
    }
}

/// The wire form of one mutation.
pub fn mutation_dto(m: &Mutation) -> p::PlanMutation {
    let (kind, bundle_id) = match m {
        Mutation::CreateCertificate { .. } => (p::MutationKind::CreateCertificate, None),
        Mutation::RegisterDevice { .. } => (p::MutationKind::RegisterDevice, None),
        Mutation::RegisterBundleId { identifier, .. } => (p::MutationKind::RegisterBundleId, Some(identifier.clone())),
        Mutation::EnableCapability { bundle, .. } => (p::MutationKind::EnableCapability, Some(bundle.clone())),
        Mutation::CreateProfile { bundle, .. } => (p::MutationKind::CreateProfile, Some(bundle.clone())),
        Mutation::DeleteProfile { .. } => (p::MutationKind::DeleteProfile, None),
    };
    p::PlanMutation { kind, summary: m.summary(), bundle_id }
}

/// The wire form of an outcome.
pub fn outcome_dto(o: &ProvisionOutcome) -> p::ProvisionApplyResult {
    p::ProvisionApplyResult {
        identity_id: o.identity.as_ref().map(|i| i.0.clone()),
        bundles: o
            .bundles
            .iter()
            .map(|b| p::BundleProvision {
                rel_path: b.rel_path.clone(),
                bundle_id: b.bundle_id.clone(),
                profile_uuid: b.profile_uuid.clone(),
                profile_path: b.profile_path.display().to_string(),
            })
            .collect(),
    }
}
