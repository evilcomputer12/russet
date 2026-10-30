// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `provision` handlers (§8.4). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! `provision.plan` is GET-only. `provision.apply` performs a zero-mutation plan without an approval; a plan
//! with account changes needs a human approval (`perm.approvalRequired`). For an `mcp` connection the permission
//! engine already requires and validates a tier-2 approval before the handler runs; for `cli`/`ide` the handler
//! itself creates and validates a plan-bound approval ([`ApprovalSubject::Plan`]), so a forged `approval` string
//! cannot trigger the mutations. `provision.capabilities` is the §6.5 map.

use russet_account::{AccountId, AccountService, ApprovalRef, IdentityId, Mutation, ProvisionOutcome, ProvisionPlan};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::ids::ApprovalId;
use russet_core::rpc::{ClientKind, ConnInfo};
use russet_ipc::{RequestCtx, Router};
use russet_protocol::approval::{ApprovalRequiredDetails, ApprovalState, ApprovalSubject};
use russet_protocol::provision as p;
use russet_protocol::sign::Distribution as WireDistribution;

use super::account::{absolute, accounts_of, redacted};
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ProvisionPlan, _, _>(|cx: DaemonCtx, _rq, params: p::ProvisionPlanParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), plan(&svc, params).await).await
    });
    r.request::<p::ProvisionApply, _, _>(|cx: DaemonCtx, rq: RequestCtx, params: p::ProvisionApplyParams| async move {
        let svc = accounts_of(&cx)?;
        redacted(Some(&svc), apply(&cx, rq.peer.connection(), &svc, params).await).await
    });
    r.request::<p::ProvisionCapabilities, _, _>(
        |_cx: DaemonCtx, _rq, _params: p::ProvisionCapabilitiesParams| async move { Ok(capabilities()) },
    );
}

/// Apple's capabilities documentation (one page for the whole map).
const CAPABILITIES_DOCS: &str = "https://developer.apple.com/help/account/reference/supported-capabilities-ios/";

/// `provision.capabilities`: the entitlement → capability map (SIGN §6.5), the same for every paid team.
pub fn capabilities() -> p::ProvisionCapabilitiesResult {
    let capabilities = russet_sign::all_capabilities()
        .into_iter()
        .map(|c| p::Capability {
            id: c.id.to_owned(),
            title: c.title,
            entitlements: c.entitlements.iter().map(|e| (*e).to_owned()).collect(),
            free_team: c.free_team,
            needs_portal_step: c.needs_portal_step,
            docs: CAPABILITIES_DOCS.to_owned(),
        })
        .collect();
    p::ProvisionCapabilitiesResult { capabilities }
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

/// `provision.apply`. A zero-mutation plan applies with no approval. A mutating plan needs a human approval: for
/// an `mcp` connection the permission engine has already required and validated a tier-2 approval (the `approval`
/// here is that id), so the handler passes it through; for `cli`/`ide` the handler creates a plan-bound approval
/// when none is presented, and validates a presented one against the plan's digest and the caller.
pub async fn apply(
    cx: &DaemonCtx,
    conn: &ConnInfo,
    svc: &AccountService,
    params: p::ProvisionApplyParams,
) -> Result<p::ProvisionApplyResult> {
    let plan = svc.plan(&params.plan_id)?;
    let approval_id = if plan.mutations.is_empty() {
        None
    } else {
        Some(resolve_mutation_approval(cx, conn, &plan, params.approval.as_deref())?)
    };
    let outcome = svc.apply_plan(&params.plan_id, &ApprovalRef { approval_id }).await?;
    Ok(outcome_dto(&outcome))
}

/// The approval id to apply a mutating plan with, or `perm.approvalRequired`/`perm.approvalInvalid`.
fn resolve_mutation_approval(
    cx: &DaemonCtx,
    conn: &ConnInfo,
    plan: &ProvisionPlan,
    presented: Option<&str>,
) -> Result<ApprovalId> {
    // An `mcp` call is gated by the permission engine (a tier-2 method approval it already validated); trust and
    // pass the id through. `mcp` never reaches the handler without a valid approval.
    if conn.kind == ClientKind::Mcp {
        return parse_id(presented.ok_or_else(approval_missing_internal)?);
    }
    let digest = russet_sign::hex(&plan.digest);
    match presented {
        None => Err(request_plan_approval(cx, conn, plan, &digest)),
        Some(id) => {
            let id = parse_id(id)?;
            validate_plan_approval(cx, conn, plan, &digest, &id)?;
            Ok(id)
        }
    }
}

/// Creates a pending plan-bound approval and returns the `perm.approvalRequired` error to send.
fn request_plan_approval(cx: &DaemonCtx, conn: &ConnInfo, plan: &ProvisionPlan, digest: &str) -> Error {
    let summary = format!("apply provisioning plan {} ({} account change(s))", plan.plan_id.0, plan.mutations.len());
    let detail = plan.mutations.iter().map(Mutation::summary).collect::<Vec<_>>().join("; ");
    let approval = cx.approvals.request(
        conn,
        ApprovalSubject::Plan { plan_id: plan.plan_id.0.clone(), plan_digest: digest.to_owned() },
        2,
        summary.clone(),
        Some(detail),
    );
    let details = ApprovalRequiredDetails {
        approval_id: approval.approval_id.clone(),
        tool: None,
        tier: 2,
        summary,
        args_digest: digest.to_owned(),
        approve_with: format!("russet approve {} (or Approve in Russet IDE)", approval.approval_id),
        expires_at: approval.expires_at.clone(),
    };
    Error::new(codes::perm::APPROVAL_REQUIRED, format!("applying plan {} needs a human approval", plan.plan_id.0))
        .with_hint(format!(
            "Ask the user to run `russet approve {}` in a terminal or to approve it in Russet IDE, then apply \
             again with approval: \"{}\".",
            approval.approval_id, approval.approval_id
        ))
        .with_details(serde_json::to_value(details).unwrap_or_default())
}

/// Checks that `id` is a granted plan approval, bound to this plan's id and digest, from this caller.
fn validate_plan_approval(
    cx: &DaemonCtx,
    conn: &ConnInfo,
    plan: &ProvisionPlan,
    digest: &str,
    id: &ApprovalId,
) -> Result<()> {
    let invalid = |why: &str| {
        Error::new(codes::perm::APPROVAL_INVALID, format!("approval {id} {why}"))
            .with_hint("Request a new approval: apply again without `approval`, then approve it.")
    };
    let approval = cx.approvals.get(id).map_err(|_| invalid("does not exist"))?;
    if approval.state != ApprovalState::Granted {
        return Err(invalid("is not granted"));
    }
    match &approval.subject {
        ApprovalSubject::Plan { plan_id, plan_digest } if plan_id == &plan.plan_id.0 && plan_digest == digest => {}
        ApprovalSubject::Plan { .. } => return Err(invalid("was granted for another plan")),
        ApprovalSubject::Method { .. } => return Err(invalid("is not a provisioning-plan approval")),
    }
    if approval.requested_by.kind != conn.kind || approval.requested_by.name != conn.name {
        return Err(invalid("belongs to another client"));
    }
    Ok(())
}

fn parse_id(s: &str) -> Result<ApprovalId> {
    ApprovalId::parse(s).map_err(|_| {
        Error::new(codes::core::INVALID_ARGUMENT, "approval must be an approval id (`apr_…`)")
            .with_hint("Pass the approvalId that perm.approvalRequired returned (`russet approvals list`).")
    })
}

/// An `mcp` call that reached the handler without an approval would be a permission-engine bug.
fn approval_missing_internal() -> Error {
    Error::new(codes::core::INTERNAL, "an MCP provision.apply reached the handler without an approval")
}

/// The wire form of a plan: the team masked, the identity named by its certificate serial, UDIDs already redacted
/// by the planner.
pub fn plan_dto(plan: &ProvisionPlan) -> p::ProvisionPlanResult {
    p::ProvisionPlanResult {
        plan_id: plan.plan_id.0.clone(),
        digest: russet_sign::hex(&plan.digest),
        account_id: plan.account_id.0.clone(),
        team_id: russet_sign::mask_team(&plan.team_id),
        distribution: wire_distribution(plan.distribution),
        identity_id: plan.identity.as_ref().map(IdentityId::wire),
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
        identity_id: o.identity.as_ref().map(IdentityId::wire),
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
