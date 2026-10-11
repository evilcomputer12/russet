// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet doctor [--fix] [--online] [--strict] [--check ID]… [--category C]…` (ARCHITECTURE §8.7; core §11).
//! Owner: W3.
//!
//! Exit status (core §11, ARCHITECTURE §13.4): 0 without failures; 4 when a check fails, or, with `--strict`,
//! warns. `--fix` runs the automatic fixes that need no root (class `system`); fixes that need root are printed as
//! commands in each check's `fix`.

use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::doctor as p;

use super::config::{project_param, report};
use crate::CliCtx;

/// The `doctor` command.
pub fn command() -> Command {
    Command::new("doctor")
        .about("Check the environment: toolchains, SDK, device, accounts [--fix] [--online] [--strict] [--check ID]")
        .arg(Arg::new("fix").long("fix").action(ArgAction::SetTrue).help("Apply the automatic fixes (no root needed)"))
        .arg(
            Arg::new("online")
                .long("online")
                .action(ArgAction::SetTrue)
                .help("Also run the checks that need the network"),
        )
        .arg(Arg::new("strict").long("strict").action(ArgAction::SetTrue).help("Exit 4 on warnings too"))
        .arg(
            Arg::new("check")
                .long("check")
                .value_name("ID")
                .action(ArgAction::Append)
                .help("Run only this check (or `prefix.` for a group); repeatable"),
        )
        .arg(Arg::new("category").long("category").value_name("CATEGORY").action(ArgAction::Append).value_parser([
            "host",
            "daemon",
            "config",
            "secrets",
            "usbmux",
            "device",
            "toolchain",
            "account",
            "network",
            "platform",
        ]))
}

/// The `doctor.run` params of an invocation, and whether `--strict` was given.
pub(crate) fn request(
    m: &ArgMatches,
    project: Option<String>,
    udid: Option<String>,
) -> Result<(p::DoctorRunParams, bool)> {
    let Some(("doctor", a)) = m.subcommand() else {
        return Err(
            Error::new(codes::core::INVALID_ARGUMENT, "not a doctor command").with_hint("Run `russet doctor --help`.")
        );
    };
    let checks: Vec<String> = a.get_many::<String>("check").map(|v| v.cloned().collect()).unwrap_or_default();
    let categories: Vec<p::DoctorCategory> = a
        .get_many::<String>("category")
        .map(|v| v.filter_map(|c| serde_json::from_value(serde_json::Value::String(c.clone())).ok()).collect())
        .unwrap_or_default();
    let params = p::DoctorRunParams {
        checks: (!checks.is_empty()).then_some(checks),
        categories: (!categories.is_empty()).then_some(categories),
        project,
        udid,
        online: a.get_flag("online").then_some(true),
        fix: a.get_flag("fix").then_some(true),
    };
    Ok((params, a.get_flag("strict")))
}

/// The exit status of a report (core §11).
pub(crate) fn exit_status(report: &p::DoctorReport, strict: bool) -> u8 {
    if report.summary.fail > 0 || (strict && report.summary.warn > 0) { 4 } else { 0 }
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let (params, strict) = request(m, project_param(&cx.global)?, cx.global.device.clone())?;
    let report_value = cx.peer.call::<p::DoctorRun>(params).await?;
    report(cx, &report_value)?;
    Ok(ExitCode::from(exit_status(&report_value, strict)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocations_become_params() {
        let m = crate::root_command()
            .try_get_matches_from([
                "russet",
                "--json",
                "doctor",
                "--fix",
                "--strict",
                "--check",
                "toolchain.",
                "--check",
                "secrets.backend",
            ])
            .expect("parses");
        let (params, strict) = request(&m, None, Some("iphone12".into())).expect("request");
        assert!(strict);
        assert_eq!(params.fix, Some(true));
        assert_eq!(params.online, None);
        assert_eq!(params.checks, Some(vec!["toolchain.".to_owned(), "secrets.backend".to_owned()]));
        assert_eq!(params.udid.as_deref(), Some("iphone12"));
        let m = crate::root_command()
            .try_get_matches_from(["russet", "doctor", "--category", "toolchain"])
            .expect("parses");
        let (params, strict) = request(&m, None, None).expect("request");
        assert_eq!((params.categories, params.fix, strict), (Some(vec![p::DoctorCategory::Toolchain]), None, false));
    }

    #[test]
    fn failures_exit_4_and_strict_counts_warnings() {
        let report = |fail, warn| p::DoctorReport {
            russet: p::DoctorBuild { version: "0.1.0".into(), build_id: "0.1.0".into() },
            host: p::DoctorHost { os: "linux".into(), distro: None, arch: "x86_64".into(), wsl: false },
            checks: vec![],
            summary: p::DoctorSummary { pass: 3, warn, fail, skip: 1 },
        };
        assert_eq!(exit_status(&report(0, 0), false), 0);
        assert_eq!(exit_status(&report(1, 0), false), 4);
        assert_eq!(exit_status(&report(0, 2), false), 0);
        assert_eq!(exit_status(&report(0, 2), true), 4);
    }
}
