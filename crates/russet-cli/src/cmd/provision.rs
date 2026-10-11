// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet provision` (ARCHITECTURE §8.7). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! Plans provisioning for the project (`provision.plan`, GET-only) and, without `--dry-run`, applies a plan
//! that changes nothing on the account (`provision.apply`: it caches the profiles `russet sign` uses). A plan
//! with account changes needs an approval and arrives with W12 (the daemon answers `perm.approvalRequired`,
//! exit 11). `--json` prints the plan (`{planId, mutations[], profiles[], …}`), plus `outcome` when applied.

use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
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
    let project = cx.global.project.as_ref().map(|p| p.display().to_string());
    let params = plan_params(sub, project.as_deref(), cx.global.device.as_deref())?;
    let plan = cx.peer.call::<p::ProvisionPlan>(params).await?;
    let mut value = serde_json::to_value(&plan)
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode the plan: {e}")))?;
    if !sub.get_flag("dry-run") {
        let outcome = cx
            .peer
            .call::<p::ProvisionApply>(p::ProvisionApplyParams { plan_id: plan.plan_id.clone(), approval: None })
            .await?;
        value["outcome"] = serde_json::to_value(&outcome)
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode the outcome: {e}")))?;
    }
    cx.reporter.result(&value);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
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
