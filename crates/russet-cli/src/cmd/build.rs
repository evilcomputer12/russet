// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet build`, `russet clean`, `russet build-settings` (ARCHITECTURE §8.7). Owner: W5 (Wave 1), W16
//! (build-settings, Wave 2).
//!
//! ```text
//! russet build [--release | --profile P] [--target T] [--ipa[=dev|store]] [--backend B] [--clean] [--set K=V]…
//!              [--dry-run]                                   build.start (build.plan with --dry-run)
//! russet clean [--all]                                       build.clean
//! ```
//!
//! The project is `--project` (global) or the current directory; the daemon finds the root upwards from it.
//! `--json` prints the `BuildOutput` (`.app.path`, `.signingPlan`, `.reportPath`, …) as one document.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::build::{self as p, BuildBackend, PackageKind};

use crate::CliCtx;

/// This area's top-level commands, as a [`super::group`].
pub fn command() -> Command {
    super::group(
        "build",
        [build_command(), clean_command(), super::stub("build-settings", "Show the resolved build settings")],
    )
}

fn build_command() -> Command {
    Command::new("build")
        .about("Build the project's iOS app")
        .arg(
            Arg::new("release")
                .long("release")
                .action(ArgAction::SetTrue)
                .conflicts_with("profile")
                .help("Build the release profile"),
        )
        .arg(
            Arg::new("profile")
                .long("profile")
                .value_name("P")
                .help("Build profile: debug, release, profile or [build.profiles.<P>]"),
        )
        .arg(
            Arg::new("target")
                .long("target")
                .visible_alias("product")
                .value_name("T")
                .help("The app product (default: [build.swiftpm] product)"),
        )
        .arg(
            Arg::new("ipa")
                .long("ipa")
                .value_name("KIND")
                .num_args(0..=1)
                .require_equals(true)
                .default_missing_value("dev")
                .value_parser(["dev", "store"])
                .help("Also package an IPA: dev (stored) or store (deflated, SwiftSupport)"),
        )
        .arg(
            Arg::new("backend")
                .long("backend")
                .value_name("B")
                .value_parser(["swiftbuild", "native", "xtool", "pif"])
                .help("SwiftPM backend"),
        )
        .arg(
            Arg::new("clean")
                .long("clean")
                .action(ArgAction::SetTrue)
                .help("Remove the SwiftPM scratch and this profile's outputs first"),
        )
        .arg(
            Arg::new("set")
                .long("set")
                .value_name("K=V")
                .action(ArgAction::Append)
                .help("Build-setting override (repeatable)"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Plan only (build.plan): nothing runs, nothing is written"),
        )
}

fn clean_command() -> Command {
    Command::new("clean")
        .about("Remove build outputs: .russet/work and .russet/build")
        .arg(Arg::new("all").long("all").action(ArgAction::SetTrue).help("Remove all of .russet, logs included"))
}

/// The project directory: `--project`, else the current directory, made absolute.
pub(crate) fn project_dir(cx: &CliCtx) -> Result<String> {
    absolute(cx.global.project.as_deref())
}

pub(crate) fn absolute(p: Option<&Path>) -> Result<String> {
    let cwd = std::env::current_dir().map_err(|e| Error::new(codes::core::IO, format!("no current directory: {e}")))?;
    let path: PathBuf = match p {
        Some(p) if p.is_absolute() => p.to_owned(),
        Some(p) => cwd.join(p),
        None => cwd,
    };
    path.into_os_string().into_string().map_err(|_| {
        Error::new(codes::core::INVALID_ARGUMENT, "the project path is not UTF-8").with_hint("Use a UTF-8 path.")
    })
}

/// `build.start` params from `russet build`'s matches.
pub fn start_params(m: &ArgMatches, project: String) -> Result<p::BuildStartParams> {
    let configuration =
        if m.get_flag("release") { Some("release".to_owned()) } else { m.get_one::<String>("profile").cloned() };
    let package =
        m.get_one::<String>("ipa").map(|k| if k == "store" { PackageKind::IpaStore } else { PackageKind::IpaDev });
    let backend = m.get_one::<String>("backend").map(|b| match b.as_str() {
        "native" => BuildBackend::Native,
        "xtool" => BuildBackend::Xtool,
        "pif" => BuildBackend::Pif,
        _ => BuildBackend::Swiftbuild,
    });
    let mut overrides = BTreeMap::new();
    for kv in m.get_many::<String>("set").into_iter().flatten() {
        let (k, v) = kv.split_once('=').filter(|(k, _)| !k.is_empty()).ok_or_else(|| {
            Error::new(codes::core::INVALID_ARGUMENT, format!("`--set {kv}` is not KEY=VALUE"))
                .with_hint("For example --set SWIFT_ACTIVE_COMPILATION_CONDITIONS=DEBUG")
        })?;
        overrides.insert(k.to_owned(), v.to_owned());
    }
    Ok(p::BuildStartParams {
        project,
        target: m.get_one::<String>("target").cloned(),
        configuration,
        package,
        backend,
        overrides: (!overrides.is_empty()).then_some(overrides),
        clean: m.get_flag("clean").then_some(true),
        bundle_id_policy: None,
    })
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot render the result: {e}")))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    match m.subcommand() {
        Some(("build", sub)) => {
            let params = start_params(sub, project_dir(cx)?)?;
            if sub.get_flag("dry-run") {
                let plan = cx.peer.call::<p::BuildPlan>(params).await?;
                cx.reporter.result(&to_json(&plan)?);
            } else {
                let out = cx.peer.call::<p::BuildStart>(params).await?;
                cx.reporter.result(&to_json(&out)?);
            }
            Ok(ExitCode::SUCCESS)
        }
        Some(("clean", sub)) => {
            let scope = if sub.get_flag("all") { p::CleanScope::All } else { p::CleanScope::Build };
            let out = cx.peer.call::<p::BuildClean>(p::BuildCleanParams { project: project_dir(cx)?, scope }).await?;
            cx.reporter.result(&to_json(&out)?);
            Ok(ExitCode::SUCCESS)
        }
        _ => super::not_implemented(m),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn sub(args: &[&str]) -> ArgMatches {
        let mut full = vec!["russet"];
        full.extend_from_slice(args);
        let m = crate::root_command().try_get_matches_from(full).unwrap();
        m.subcommand().unwrap().1.clone()
    }

    #[test]
    fn build_flags_become_params() {
        let m = sub(&[
            "build",
            "--release",
            "--ipa=store",
            "--backend",
            "native",
            "--set",
            "A=1",
            "--set",
            "B=x=y",
            "--clean",
            "--target",
            "App",
        ]);
        let p = start_params(&m, "/p".into()).unwrap();
        assert_eq!(p.configuration.as_deref(), Some("release"));
        assert_eq!(p.package, Some(PackageKind::IpaStore));
        assert_eq!(p.backend, Some(BuildBackend::Native));
        assert_eq!(p.clean, Some(true));
        assert_eq!(p.target.as_deref(), Some("App"));
        let o = p.overrides.unwrap();
        assert_eq!(o.get("A").map(String::as_str), Some("1"));
        assert_eq!(o.get("B").map(String::as_str), Some("x=y"));
    }

    #[test]
    fn defaults_send_nothing_extra() {
        let p = start_params(&sub(&["build"]), "/p".into()).unwrap();
        assert_eq!(serde_json::to_value(&p).unwrap(), serde_json::json!({"project": "/p"}));
        let p = start_params(&sub(&["build", "--ipa"]), "/p".into()).unwrap();
        assert_eq!(p.package, Some(PackageKind::IpaDev));
        let p = start_params(&sub(&["build", "--profile", "staging"]), "/p".into()).unwrap();
        assert_eq!(p.configuration.as_deref(), Some("staging"));
    }

    #[test]
    fn bad_flags_are_refused() {
        assert!(
            crate::root_command().try_get_matches_from(["russet", "build", "--release", "--profile", "x"]).is_err()
        );
        assert!(crate::root_command().try_get_matches_from(["russet", "build", "--ipa=zip"]).is_err());
        let e = start_params(&sub(&["build", "--set", "NOEQUALS"]), "/p".into()).unwrap_err();
        assert_eq!(e.code(), codes::core::INVALID_ARGUMENT);
        assert!(crate::root_command().try_get_matches_from(["russet", "clean", "--all"]).is_ok());
    }

    #[test]
    fn relative_projects_become_absolute() {
        let a = absolute(Some(Path::new("sub"))).unwrap();
        assert!(Path::new(&a).is_absolute() && a.ends_with("sub"));
    }
}
