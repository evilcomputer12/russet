// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet provision` (ARCHITECTURE §8.7). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! Plans provisioning for the project (`provision.plan`, GET-only) and, without `--dry-run`, applies it
//! (`provision.apply`: it caches the profiles `russet sign` uses). A plan with account changes needs a person's
//! approval (ARCHITECTURE §11.3): in a terminal the command lists the changes and asks; an agent's shell gets
//! `perm.approvalRequired` (exit 11) with the plan and approval ids, and finishes with `--plan pln_… --approval
//! apr_…` once a person approved (Russet IDE asks them, or `russet approve`). `--json` prints the plan
//! (`{planId, mutations[], profiles[], …}`), plus `outcome` when applied.

use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::ids::ApprovalId;
use russet_core::platform::{read_line_from_tty, stderr_is_tty, stdin_is_tty};
use russet_protocol::approval::{ApprovalGrant, ApprovalGrantParams, ApprovalScope};
use russet_protocol::provision as p;
use russet_protocol::sign::Distribution;

use super::account::absolute_path;
use crate::CliCtx;

/// The `provision` command.
pub fn command() -> Command {
    Command::new("provision")
        .about("Provision signing assets [--distribution development|ad-hoc|app-store-connect] [--dry-run]")
        .arg(
            Arg::new("distribution")
                .long("distribution")
                .value_name("KIND")
                .value_parser(["development", "ad-hoc", "app-store-connect"])
                .default_value("development"),
        )
        .arg(Arg::new("dry-run").long("dry-run").action(ArgAction::SetTrue).help("Plan only (read-only)"))
        .arg(
            Arg::new("account")
                .long("account")
                .value_name("ID")
                .help("The account (default: [signing] account, else the default one)"),
        )
        .arg(
            Arg::new("udid")
                .long("udid")
                .value_name("UDID")
                .action(ArgAction::Append)
                .help("A device the profile must list (repeatable; also the global --device)"),
        )
        .arg(
            Arg::new("plan")
                .long("plan")
                .value_name("PLAN")
                .requires("approval")
                .help("Apply this plan (`pln_…`, from an earlier `russet provision`) instead of planning again"),
        )
        .arg(
            Arg::new("approval")
                .long("approval")
                .value_name("ID")
                .requires("plan")
                .help("The approval (`apr_…`) a person granted for the plan's account changes"),
        )
}

/// The wire distribution of a CLI value.
pub fn parse_distribution(s: &str) -> Result<Distribution> {
    match s {
        "development" => Ok(Distribution::Development),
        "ad-hoc" => Ok(Distribution::AdHoc),
        "app-store-connect" => Ok(Distribution::AppStoreConnect),
        other => Err(Error::new(codes::core::INVALID_ARGUMENT, format!("unknown distribution `{other}`"))),
    }
}

/// The `provision.plan` params of an invocation: the project from `--project` (else the working directory),
/// devices from `--udid` and the global `--device`.
pub fn plan_params(m: &ArgMatches, project: Option<&str>, device: Option<&str>) -> Result<p::ProvisionPlanParams> {
    let project = absolute_path(project.unwrap_or("."))?;
    let mut devices: Vec<String> = m.get_many::<String>("udid").map(|v| v.cloned().collect()).unwrap_or_default();
    if let Some(d) = device
        && !devices.iter().any(|x| x == d)
    {
        devices.push(d.to_owned());
    }
    Ok(p::ProvisionPlanParams {
        project,
        distribution: Some(parse_distribution(
            m.get_one::<String>("distribution").map_or("development", String::as_str),
        )?),
        devices: (!devices.is_empty()).then_some(devices),
        account_id: m.get_one::<String>("account").cloned(),
    })
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("provision", sub)) = m.subcommand() else {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, "not a provision command"));
    };
    // The second half of an approved plan: a person granted its approval (Russet IDE, or `russet approve`).
    if let (Some(plan_id), Some(approval)) = (sub.get_one::<String>("plan"), sub.get_one::<String>("approval")) {
        let params = p::ProvisionApplyParams { plan_id: plan_id.clone(), approval: Some(approval.clone()) };
        let outcome = cx.peer.call::<p::ProvisionApply>(params).await?;
        cx.reporter.result(&serde_json::json!({ "planId": plan_id, "outcome": json(&outcome)? }));
        return Ok(ExitCode::SUCCESS);
    }
    let project = cx.global.project.as_ref().map(|p| p.display().to_string());
    let params = plan_params(sub, project.as_deref(), cx.global.device.as_deref())?;
    let plan = cx.peer.call::<p::ProvisionPlan>(params).await?;
    let mut value = json(&plan)?;
    if !sub.get_flag("dry-run") {
        let apply = |approval: Option<String>| {
            cx.peer.call::<p::ProvisionApply>(p::ProvisionApplyParams { plan_id: plan.plan_id.clone(), approval })
        };
        let outcome = match apply(None).await {
            Err(e) if e.code() == codes::perm::APPROVAL_REQUIRED => {
                // Account changes need a person's approval (ARCHITECTURE §11.3): asked here in a terminal; an
                // agent's shell gets how to finish once a person approved.
                let Some(id) =
                    e.details().and_then(|d| d.get("approvalId")).and_then(|v| v.as_str()).map(str::to_owned)
                else {
                    return Err(e);
                };
                if !(stdin_is_tty() && stderr_is_tty()) {
                    return Err(e.with_hint(format!(
                        "A person approves account changes: Russet IDE asks them, or they run `russet approve {id}` \
                         in a terminal. Then run `russet provision --plan {} --approval {id}` (within 10 minutes).",
                        plan.plan_id
                    )));
                }
                if !confirm_changes(&plan)? {
                    return Err(Error::cancelled().with_hint("Nothing was changed in your Apple developer account."));
                }
                let approval_id = ApprovalId::parse(&id)?;
                cx.peer
                    .call::<ApprovalGrant>(ApprovalGrantParams { approval_id, scope: Some(ApprovalScope::Once) })
                    .await?;
                apply(Some(id)).await?
            }
            other => other?,
        };
        value["outcome"] = json(&outcome)?;
    }
    cx.reporter.result(&value);
    Ok(ExitCode::SUCCESS)
}

/// A result as JSON for the reporter.
fn json(v: &impl serde::Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode the result: {e}")))
}

/// Lists a plan's account changes on the terminal and asks whether to make them.
fn confirm_changes(plan: &p::ProvisionPlanResult) -> Result<bool> {
    let n = plan.mutations.len();
    let mut question =
        format!("This needs {n} change{} in your Apple developer account:\n", if n == 1 { "" } else { "s" });
    for m in &plan.mutations {
        question.push_str(&format!("  • {}\n", m.summary));
    }
    question.push_str("Make them? [y/N] ");
    let answer = read_line_from_tty(&question, false).map_err(|e| Error::io("reading the answer", e))?;
    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    #[cfg_attr(windows, ignore = "Unix path expectations; the engine runs in Linux (WSL on Windows)")]
    fn builds_plan_params() {
        command().debug_assert();
        let m =
            command().try_get_matches_from(["provision", "--dry-run", "--udid", "A", "--account", "asc:K"]).unwrap();
        let p = plan_params(&m, Some("/proj"), Some("B")).unwrap();
        assert_eq!(p.project, "/proj");
        assert_eq!(p.distribution, Some(Distribution::Development));
        assert_eq!(p.devices, Some(vec!["A".to_owned(), "B".to_owned()]));
        assert_eq!(p.account_id.as_deref(), Some("asc:K"));
        let m = command().try_get_matches_from(["provision", "--distribution", "app-store-connect"]).unwrap();
        let p = plan_params(&m, None, None).unwrap();
        assert!(std::path::Path::new(&p.project).is_absolute());
        assert_eq!(p.distribution, Some(Distribution::AppStoreConnect));
        assert_eq!(p.devices, None);
        assert!(command().try_get_matches_from(["provision", "--distribution", "nope"]).is_err());
    }
}
