// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet ship` (ARCHITECTURE §8.7; SIGN §9, §12). Owner: W12 (archive, ipa; Wave 2), W23 (the rest; Wave 4).
//!
//! - `russet ship archive [--project DIR] [--configuration C] [--method M]` → the `.xcarchive` (SIGN §9.1).
//! - `russet ship ipa <ARCHIVE|APP> [--method M] [--out FILE]` → the deterministic IPA (SIGN §9.3).
//!
//! The other publishing subcommands arrive with W23. `--json` prints the archive or IPA info.

use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::ship as p;

use super::account::absolute_path;
use crate::CliCtx;

fn method_arg(default: &'static str) -> Arg {
    Arg::new("method")
        .long("method")
        .value_name("M")
        .value_parser(["development", "ad-hoc", "app-store-connect", "enterprise"])
        .default_value(default)
        .help("The export method (SIGN §9.2)")
}

/// The `ship` command.
pub fn command() -> Command {
    Command::new("ship")
        .about(
            "Publishing: archive, ipa (W12); stamp, validate, upload, testflight, metadata, version, review, ci (W23)",
        )
        .subcommand_required(true)
        .subcommand(
            Command::new("archive")
                .about("Build the project and lay out an .xcarchive (SIGN §9.1)")
                .arg(
                    Arg::new("project")
                        .long("project")
                        .value_name("DIR")
                        .help("The project (default: --project, else .)"),
                )
                .arg(
                    Arg::new("configuration")
                        .long("configuration")
                        .value_name("C")
                        .help("Build configuration (default: release)"),
                )
                .arg(method_arg("app-store-connect")),
        )
        .subcommand(
            Command::new("ipa")
                .about("Write a store/development IPA from an archive or a built .app (SIGN §9.3)")
                .arg(Arg::new("input").value_name("ARCHIVE|APP").required(true).help("An .xcarchive or a built .app"))
                .arg(method_arg("app-store-connect"))
                .arg(
                    Arg::new("out")
                        .long("out")
                        .value_name("FILE")
                        .help("Where to write the IPA (default: beside the input)"),
                ),
        )
        .subcommand(super::stub("stamp", "Metadata stamping (W23)"))
        .subcommand(super::stub("validate", "Offline validation (W23)"))
        .subcommand(super::stub("upload", "Upload to App Store Connect (W23)"))
        .subcommand(super::stub("testflight", "TestFlight (W23)"))
        .subcommand(super::stub("metadata", "App Store metadata (W23)"))
        .subcommand(super::stub("version", "App Store versions (W23)"))
        .subcommand(super::stub("review", "App Store review (W23)"))
        .subcommand(super::stub("ci", "Apple-hardware CI (W23)"))
}

fn json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode the result: {e}")))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("ship", sub)) = m.subcommand() else {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, "not a ship command"));
    };
    let value = match sub.subcommand() {
        Some(("archive", s)) => {
            let project = s
                .get_one::<String>("project")
                .cloned()
                .or_else(|| cx.global.project.as_ref().map(|p| p.display().to_string()))
                .unwrap_or_else(|| ".".to_owned());
            let params = p::ShipArchiveParams {
                project: absolute_path(&project)?,
                configuration: s.get_one::<String>("configuration").cloned(),
                method: s.get_one::<String>("method").cloned().unwrap_or_else(|| "app-store-connect".to_owned()),
            };
            json(&cx.peer.call::<p::ShipArchive>(params).await?)?
        }
        Some(("ipa", s)) => {
            let params = p::ShipIpaParams {
                archive_path: absolute_path(s.get_one::<String>("input").map(String::as_str).unwrap_or_default())?,
                method: s.get_one::<String>("method").cloned().unwrap_or_else(|| "app-store-connect".to_owned()),
                out: s.get_one::<String>("out").map(|o| absolute_path(o)).transpose()?,
            };
            json(&cx.peer.call::<p::ShipIpa>(params).await?)?
        }
        Some((other, _)) => {
            return Err(Error::new(codes::core::NOT_IMPLEMENTED, format!("`ship {other}` arrives with W23"))
                .with_hint("Wave 2 has `ship archive` and `ship ipa`."));
        }
        None => {
            return Err(
                Error::new(codes::core::INVALID_ARGUMENT, "no ship subcommand").with_hint("Run `russet ship --help`.")
            );
        }
    };
    cx.reporter.result(&value);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_tree_is_valid() {
        command().debug_assert();
    }

    #[test]
    fn parses_archive_and_ipa() {
        let m = command().try_get_matches_from(["ship", "archive", "--project", "/p", "--method", "ad-hoc"]).unwrap();
        let (_, s) = m.subcommand().unwrap();
        assert_eq!(s.get_one::<String>("method").map(String::as_str), Some("ad-hoc"));
        let m = command().try_get_matches_from(["ship", "ipa", "App.xcarchive", "--out", "x.ipa"]).unwrap();
        let (name, _) = m.subcommand().unwrap();
        assert_eq!(name, "ipa");
        assert!(command().try_get_matches_from(["ship", "archive", "--method", "nope"]).is_err());
        assert!(command().try_get_matches_from(["ship", "ipa"]).is_err(), "input is required");
    }
}
