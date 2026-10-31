// SPDX-License-Identifier: AGPL-3.0-only
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

use super::config::{project_param, report_as};
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

fn status_label(s: p::DoctorStatus) -> &'static str {
    match s {
        p::DoctorStatus::Pass => "ok",
        p::DoctorStatus::Warn => "warn",
        p::DoctorStatus::Fail => "FAIL",
        p::DoctorStatus::Skip => "skip",
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The human report (core §11): the checks grouped by category in report order, each failure or warning with its
/// hint and fix, then the counts.
pub(crate) fn human(r: &p::DoctorReport) -> String {
    let mut out = vec![format!(
        "Russet {} ({}) on {} {}{}{}",
        r.russet.version,
        r.russet.build_id,
        r.host.os,
        r.host.arch,
        r.host.distro.as_deref().map(|d| format!(" ({d})")).unwrap_or_default(),
        if r.host.wsl { ", WSL" } else { "" }
    )];
    let width = r.checks.iter().map(|c| c.id.len()).max().unwrap_or(0);
    let mut categories: Vec<p::DoctorCategory> = Vec::new();
    for c in &r.checks {
        if !categories.contains(&c.category) {
            categories.push(c.category);
        }
    }
    for cat in categories {
        let name = serde_json::to_value(cat).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        out.push(String::new());
        out.push(name);
        for c in r.checks.iter().filter(|c| c.category == cat) {
            out.push(format!("  {:<4}  {:<width$}  {}", status_label(c.status), c.id, c.message));
            // Under the message: "  " + the 4-wide status + "  " + the id column + "  ".
            let pad = " ".repeat(width + 10);
            let problem = matches!(c.status, p::DoctorStatus::Fail | p::DoctorStatus::Warn);
            if problem && let Some(d) = &c.detail {
                out.push(format!("{pad}{d}"));
            }
            if problem && let Some(h) = &c.hint {
                out.push(format!("{pad}hint: {h}"));
            }
            match &c.fix {
                Some(p::DoctorFix::Auto { applied: true }) => out.push(format!("{pad}fixed automatically")),
                Some(p::DoctorFix::Auto { applied: false }) => {
                    out.push(format!("{pad}fix: `russet doctor --fix --check {}`", c.id));
                }
                Some(p::DoctorFix::Command { command }) => out.push(format!("{pad}fix: `{command}`")),
                Some(p::DoctorFix::Manual) | None => {}
            }
        }
    }
    let s = &r.summary;
    out.push(String::new());
    out.push(format!(
        "{}, {}, {}, {}",
        plural(s.pass, "passed", "passed"),
        plural(s.warn, "warning", "warnings"),
        plural(s.fail, "failed", "failed"),
        plural(s.skip, "skipped", "skipped")
    ));
    out.join("\n")
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let (params, strict) = request(m, project_param(&cx.global)?, cx.global.device.clone())?;
    let report_value = cx.peer.call::<p::DoctorRun>(params).await?;
    report_as(cx, &report_value, human)?;
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

    #[test]
    fn the_human_report_groups_checks_by_category() {
        let check = |id: &str, category, status, fix| p::DoctorCheck {
            id: id.into(),
            category,
            title: id.into(),
            status,
            message: format!("{id} says hello"),
            detail: None,
            hint: (status == p::DoctorStatus::Fail).then(|| "do this".to_owned()),
            fix,
            duration_ms: 1,
        };
        let r = p::DoctorReport {
            russet: p::DoctorBuild { version: "0.1.0".into(), build_id: "0.1.0+gabc".into() },
            host: p::DoctorHost {
                os: "linux".into(),
                distro: Some("cachyos".into()),
                arch: "x86_64".into(),
                wsl: false,
            },
            checks: vec![
                check("host.os", p::DoctorCategory::Host, p::DoctorStatus::Pass, None),
                check(
                    "toolchain.lldb",
                    p::DoctorCategory::Toolchain,
                    p::DoctorStatus::Fail,
                    Some(p::DoctorFix::Auto { applied: false }),
                ),
                check("host.disk", p::DoctorCategory::Host, p::DoctorStatus::Warn, Some(p::DoctorFix::Manual)),
            ],
            summary: p::DoctorSummary { pass: 1, warn: 1, fail: 1, skip: 0 },
        };
        let text = human(&r);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Russet 0.1.0 (0.1.0+gabc) on linux x86_64 (cachyos)");
        let host = lines.iter().position(|l| *l == "host").expect("host group");
        let toolchain = lines.iter().position(|l| *l == "toolchain").expect("toolchain group");
        assert!(host < toolchain);
        assert!(lines[host + 2].contains("warn") && lines[host + 2].contains("host.disk"), "{text}");
        assert!(text.contains("FAIL  toolchain.lldb"), "{text}");
        assert!(text.contains("hint: do this") && text.contains("fix: `russet doctor --fix --check toolchain.lldb`"));
        let fail = lines.iter().position(|l| l.contains("FAIL  toolchain.lldb")).expect("the failure");
        let message_column = lines[fail].find("toolchain.lldb says hello").expect("message");
        assert_eq!(lines[fail + 1].find("hint:"), Some(message_column), "hints line up under the message:\n{text}");
        assert_eq!(lines.last().copied(), Some("1 passed, 1 warning, 1 failed, 0 skipped"));
    }
}
