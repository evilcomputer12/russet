// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet sign` (ARCHITECTURE §8.7; SIGN §12). Owner: W6.
//!
//! - `russet sign <App.app> [--distribution …] [--identity ID] [--profile REL=FILE]… [--entitlements REL=FILE]…
//!   [--merge least|profile-all] [--out PATH] [--no-fix NAME]… [--get-task-allow] [--lint P|none]`
//! - `russet sign verify|lint|inspect <App.app|App.ipa> [--profile device-install|app-store]`
//!
//! `sign verify` exits 8 (`sign.verifyFailed`) when the signature does not verify, and `sign lint` exits 8
//! (`sign.lintFailed`) on an error finding; the report is printed first either way.

use std::collections::BTreeMap;
use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::sign as p;

use super::account::absolute_path;
use super::provision::parse_distribution;
use crate::CliCtx;

fn profile_arg() -> Arg {
    Arg::new("profile")
        .long("profile")
        .value_name("PROFILE")
        .value_parser(["device-install", "app-store"])
        .help("The lint profile")
}

/// The `sign` command.
pub fn command() -> Command {
    let read = |name: &'static str, about: &'static str| {
        Command::new(name).about(about).arg(Arg::new("path").value_name("APP|IPA").required(true))
    };
    Command::new("sign")
        .about("Sign an app; sign verify|lint|inspect <path>")
        .args_conflicts_with_subcommands(true)
        .subcommand_negates_reqs(true)
        .arg(Arg::new("path").value_name("APP").required(true).help("The .app to sign"))
        .arg(
            Arg::new("distribution")
                .long("distribution")
                .value_name("KIND")
                .value_parser(["development", "ad-hoc", "app-store-connect"])
                .default_value("development"),
        )
        .arg(
            Arg::new("identity")
                .long("identity")
                .value_name("TEAM/SERIAL")
                .help("The signing identity (default: the newest the profiles list)"),
        )
        .arg(
            Arg::new("sign-profile")
                .long("profile")
                .value_name("REL=FILE")
                .action(ArgAction::Append)
                .help("A .mobileprovision for a bundle (`=FILE` or `.=FILE` for the app; else cached profiles)"),
        )
        .arg(
            Arg::new("entitlements")
                .long("entitlements")
                .value_name("REL=FILE")
                .action(ArgAction::Append)
                .help("Requested entitlements for a bundle"),
        )
        .arg(Arg::new("merge").long("merge").value_name("POLICY").value_parser(["least", "profile-all"]))
        .arg(Arg::new("out").long("out").value_name("PATH").help("Sign a copy here (default: in place)"))
        .arg(
            Arg::new("no-fix")
                .long("no-fix")
                .value_name("NAME")
                .action(ArgAction::Append)
                .help("Skip a pre-sign fix: chmod, align-string-pool, clean"),
        )
        .arg(
            Arg::new("get-task-allow")
                .long("get-task-allow")
                .action(ArgAction::SetTrue)
                .help("Sign for debugging (development profiles)"),
        )
        .arg(
            Arg::new("udid")
                .long("udid")
                .value_name("UDID")
                .help("A device the cached profiles must list (also the global --device)"),
        )
        .arg(
            Arg::new("lint")
                .long("lint")
                .value_name("PROFILE")
                .value_parser(["device-install", "app-store", "none"])
                .default_value("device-install"),
        )
        .subcommand(
            read("verify", "Verify every signature (code pages, CMS, sealed resources, profile rules)")
                .arg(profile_arg())
                .arg(Arg::new("udid").long("udid").value_name("UDID").help("A device the embedded profiles must list")),
        )
        .subcommand(
            read("lint", "Run the L-SIG, L-MACHO and L-BND lints")
                .arg(profile_arg())
                .arg(Arg::new("store").long("store").action(ArgAction::SetTrue).help("Same as --profile app-store")),
        )
        .subcommand(read("inspect", "Describe every signature (team IDs masked)"))
}

/// Parses `REL=FILE` pairs (`=FILE`, `.=FILE` and `app=FILE` name the app itself).
fn pairs(m: &ArgMatches, id: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for v in m.get_many::<String>(id).into_iter().flatten() {
        let (rel, file) = v.split_once('=').ok_or_else(|| {
            Error::new(codes::core::INVALID_ARGUMENT, format!("`{v}` is not REL=FILE")).with_hint(
                "For example `--profile =app.mobileprovision --profile PlugIns/Widget.appex=widget.mobileprovision`.",
            )
        })?;
        let rel = if matches!(rel, "" | "." | "app") { String::new() } else { rel.trim_end_matches('/').to_owned() };
        out.insert(rel, absolute_path(file)?);
    }
    Ok(out)
}

fn lint_profile(s: Option<&str>) -> Option<p::LintProfile> {
    match s {
        Some("device-install") => Some(p::LintProfile::DeviceInstall),
        Some("app-store") => Some(p::LintProfile::AppStore),
        _ => None,
    }
}

/// What one `russet sign …` invocation asks the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignCall {
    Run(Box<p::SignRunParams>),
    Verify(p::SignPathParams),
    Lint(p::SignPathParams),
    Inspect(p::SignPathParams),
}

/// Maps the `sign` matches (and the global `--device`) to a daemon call.
pub fn call(m: &ArgMatches, device: Option<&str>) -> Result<SignCall> {
    let path_of = |s: &ArgMatches| absolute_path(s.get_one::<String>("path").map(String::as_str).unwrap_or_default());
    Ok(match m.subcommand() {
        Some(("verify", s)) => SignCall::Verify(p::SignPathParams {
            path: path_of(s)?,
            profile: lint_profile(s.get_one::<String>("profile").map(String::as_str)),
            device: s.get_one::<String>("udid").cloned().or_else(|| device.map(str::to_owned)),
        }),
        Some(("lint", s)) => {
            let profile = if s.get_flag("store") {
                Some("app-store")
            } else {
                s.get_one::<String>("profile").map(String::as_str)
            };
            SignCall::Lint(p::SignPathParams { path: path_of(s)?, profile: lint_profile(profile), device: None })
        }
        Some(("inspect", s)) => SignCall::Inspect(p::SignPathParams { path: path_of(s)?, profile: None, device: None }),
        _ => {
            let lint = m.get_one::<String>("lint").map(String::as_str);
            let mut no_fix: Vec<String> =
                m.get_many::<String>("no-fix").map(|v| v.cloned().collect()).unwrap_or_default();
            no_fix.sort();
            no_fix.dedup();
            SignCall::Run(Box::new(p::SignRunParams {
                path: path_of(m)?,
                distribution: parse_distribution(
                    m.get_one::<String>("distribution").map_or("development", String::as_str),
                )?,
                identity_id: m.get_one::<String>("identity").cloned(),
                profiles: pairs(m, "sign-profile")?,
                entitlements: pairs(m, "entitlements")?,
                merge_policy: match m.get_one::<String>("merge").map(String::as_str) {
                    Some("profile-all") => Some(p::MergePolicy::ProfileAll),
                    Some(_) => Some(p::MergePolicy::Least),
                    None => None,
                },
                output: m.get_one::<String>("out").map(|o| absolute_path(o)).transpose()?,
                get_task_allow: m.get_flag("get-task-allow").then_some(true),
                device: m.get_one::<String>("udid").cloned().or_else(|| device.map(str::to_owned)),
                no_fix,
                lint: lint_profile(lint),
                no_lint: (lint == Some("none")).then_some(true),
            }))
        }
    })
}

fn json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode the result: {e}")))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("sign", sub)) = m.subcommand() else {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, "not a sign command"));
    };
    match call(sub, cx.global.device.as_deref())? {
        SignCall::Run(params) => {
            let r = cx.peer.call::<p::SignRun>(*params).await?;
            cx.reporter.result(&json(&r)?);
        }
        SignCall::Verify(params) => {
            let r = cx.peer.call::<p::SignVerify>(params).await?;
            cx.reporter.result(&json(&r)?);
            if !r.ok {
                let first = r.findings.iter().find(|f| f.severity == p::Severity::Error);
                return Err(Error::new(
                    codes::sign::VERIFY_FAILED,
                    format!(
                        "the signature does not verify: {}",
                        first.map_or("see the findings", |f| f.message.as_str())
                    ),
                )
                .with_hint("Re-sign with `russet sign`; the findings name each broken file."));
            }
        }
        SignCall::Lint(params) => {
            let r = cx.peer.call::<p::SignLint>(params).await?;
            cx.reporter.result(&json(&r)?);
            let errors = r.findings.iter().filter(|f| f.severity == p::Severity::Error).count();
            if errors > 0 {
                return Err(Error::new(codes::sign::LINT_FAILED, format!("{errors} lint error(s)"))
                    .with_hint("Fix the findings above; warnings do not block."));
            }
        }
        SignCall::Inspect(params) => {
            let r = cx.peer.call::<p::SignInspect>(params).await?;
            cx.reporter.result(&json(&r)?);
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parse(args: &[&str], device: Option<&str>) -> Result<SignCall> {
        let mut argv = vec!["sign"];
        argv.extend_from_slice(args);
        call(&command().try_get_matches_from(argv).unwrap(), device)
    }

    #[test]
    fn the_tree_is_valid() {
        command().debug_assert();
    }

    #[test]
    fn maps_sign_and_its_subcommands() {
        match parse(
            &[
                "Probe.app",
                "--profile",
                "=/p/app.mobileprovision",
                "--profile",
                "PlugIns/W.appex/=/p/w.mobileprovision",
                "--get-task-allow",
                "--no-fix",
                "chmod",
                "--lint",
                "none",
            ],
            Some("UDID"),
        )
        .unwrap()
        {
            SignCall::Run(p) => {
                assert!(std::path::Path::new(&p.path).is_absolute());
                assert_eq!(p.profiles.get("").map(String::as_str), Some("/p/app.mobileprovision"));
                assert_eq!(p.profiles.get("PlugIns/W.appex").map(String::as_str), Some("/p/w.mobileprovision"));
                assert_eq!(p.get_task_allow, Some(true));
                assert_eq!(p.no_fix, vec!["chmod"]);
                assert_eq!(p.device.as_deref(), Some("UDID"));
                assert_eq!((p.lint, p.no_lint), (None, Some(true)));
                assert_eq!(p.distribution, p::Distribution::Development);
            }
            other => panic!("{other:?}"),
        }
        match parse(&["Probe.app"], None).unwrap() {
            SignCall::Run(p) => assert_eq!(p.lint, Some(p::LintProfile::DeviceInstall)),
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(parse(&["verify", "/a/App.ipa", "--profile", "app-store"], None).unwrap(), SignCall::Verify(p) if p.profile == Some(p::LintProfile::AppStore))
        );
        assert!(
            matches!(parse(&["lint", "/a/App.app", "--store"], None).unwrap(), SignCall::Lint(p) if p.profile == Some(p::LintProfile::AppStore))
        );
        assert!(matches!(parse(&["inspect", "/a/App.app"], None).unwrap(), SignCall::Inspect(_)));
        assert_eq!(
            parse(&["Probe.app", "--profile", "nopair"], None).unwrap_err().code(),
            codes::core::INVALID_ARGUMENT
        );
        assert!(command().try_get_matches_from(["sign"]).is_err(), "a path is required");
    }
}
