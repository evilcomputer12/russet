// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet project` (ARCHITECTURE §8.7). Owner: W5 (Wave 1), W16 (init, Wave 2).
//!
//! ```text
//! russet project detect                                      project.detect
//! russet project info                                        project.info
//! russet project info-plist [--target T] [--configuration C] project.infoPlist
//! russet project init …                                      project.init (W16)
//! ```

use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::project as p;

use super::build::project_dir;
use crate::CliCtx;

/// The `project` command.
pub fn command() -> Command {
    Command::new("project")
        .about("Projects: detect, info, info-plist, init")
        .subcommand_required(true)
        .subcommand(Command::new("detect").about("Detect the project around the current directory (or --project)"))
        .subcommand(Command::new("info").about("Targets, configurations and the default target"))
        .subcommand(
            Command::new("info-plist")
                .about("The Info.plist a build would write, with each key's source")
                .arg(
                    Arg::new("target").long("target").value_name("T").help("The app (default) or an extension product"),
                )
                .arg(
                    Arg::new("configuration")
                        .long("configuration")
                        .value_name("C")
                        .help("debug (default), release, profile or a named profile"),
                ),
        )
        .subcommand(super::stub("init", "Create a project (W16)"))
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot render the result: {e}")))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("project", pm)) = m.subcommand() else { return super::not_implemented(m) };
    let project = project_dir(cx)?;
    let value = match pm.subcommand() {
        Some(("detect", _)) => {
            to_json(&cx.peer.call::<p::ProjectDetect>(p::ProjectDetectParams { dir: project }).await?)?
        }
        Some(("info", _)) => to_json(&cx.peer.call::<p::ProjectInfo>(p::ProjectInfoParams { project }).await?)?,
        Some(("info-plist", sub)) => {
            let params = p::ProjectInfoPlistParams {
                project,
                target: sub.get_one::<String>("target").cloned(),
                configuration: sub.get_one::<String>("configuration").cloned(),
            };
            to_json(&cx.peer.call::<p::ProjectInfoPlist>(params).await?)?
        }
        _ => return super::not_implemented(m),
    };
    cx.reporter.result(&value);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    #[test]
    fn the_tree_parses() {
        let m =
            crate::root_command().try_get_matches_from(["russet", "project", "info-plist", "--target", "W"]).unwrap();
        let (_, pm) = m.subcommand().unwrap();
        let (name, sub) = pm.subcommand().unwrap();
        assert_eq!(name, "info-plist");
        assert_eq!(sub.get_one::<String>("target").map(String::as_str), Some("W"));
        assert!(crate::root_command().try_get_matches_from(["russet", "project"]).is_err(), "a subcommand is required");
    }
}
