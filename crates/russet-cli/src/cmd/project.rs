// SPDX-License-Identifier: AGPL-3.0-only
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

use clap::{Arg, ArgAction, ArgMatches, Command};
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
        .subcommand(
            Command::new("init")
                .about("Create a new app project from a template")
                .arg(Arg::new("name").value_name("NAME").required(true).help("The app's display name"))
                .arg(Arg::new("dir").long("dir").value_name("DIR").help("Target directory (default: ./<name>)"))
                .arg(
                    Arg::new("stack")
                        .long("stack")
                        .value_name("STACK")
                        .default_value("swiftpm")
                        .value_parser(["swiftpm", "flutter", "xcode", "react-native", "maui"])
                        .help("The project stack (only swiftpm is supported today)"),
                )
                .arg(
                    Arg::new("bundle-id")
                        .long("bundle-id")
                        .value_name("ID")
                        .help("Bundle identifier (default: com.example.<name>)"),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Write into the directory even if it already has files"),
                ),
        )
}

/// Maps the `--stack` string (already restricted by the value parser) to the wire enum.
fn parse_stack(s: &str) -> Result<p::ProjectStack> {
    Ok(match s {
        "swiftpm" => p::ProjectStack::Swiftpm,
        "flutter" => p::ProjectStack::Flutter,
        "xcode" => p::ProjectStack::Xcode,
        "react-native" => p::ProjectStack::ReactNative,
        "maui" => p::ProjectStack::Maui,
        other => {
            return Err(Error::new(codes::build::SCAFFOLD_STACK_UNSUPPORTED, format!("unknown stack {other:?}")));
        }
    })
}

/// Resolves a user path to an absolute string against the current directory.
fn absolutize(d: &str) -> Result<String> {
    let p = std::path::Path::new(d);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot read the current directory: {e}")))?
            .join(p)
    };
    Ok(abs.to_string_lossy().into_owned())
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot render the result: {e}")))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("project", pm)) = m.subcommand() else { return super::not_implemented(m) };
    let value = match pm.subcommand() {
        Some(("detect", _)) => {
            to_json(&cx.peer.call::<p::ProjectDetect>(p::ProjectDetectParams { dir: project_dir(cx)? }).await?)?
        }
        Some(("info", _)) => {
            to_json(&cx.peer.call::<p::ProjectInfo>(p::ProjectInfoParams { project: project_dir(cx)? }).await?)?
        }
        Some(("info-plist", sub)) => {
            let params = p::ProjectInfoPlistParams {
                project: project_dir(cx)?,
                target: sub.get_one::<String>("target").cloned(),
                configuration: sub.get_one::<String>("configuration").cloned(),
            };
            to_json(&cx.peer.call::<p::ProjectInfoPlist>(params).await?)?
        }
        Some(("init", sub)) => {
            let name = sub.get_one::<String>("name").cloned().unwrap_or_default();
            let stack = parse_stack(sub.get_one::<String>("stack").map(String::as_str).unwrap_or("swiftpm"))?;
            let dir = match sub.get_one::<String>("dir") {
                Some(d) => absolutize(d)?,
                None => absolutize(&name)?,
            };
            let params = p::ProjectInitParams {
                dir,
                stack,
                name,
                bundle_id: sub.get_one::<String>("bundle-id").cloned(),
                force: sub.get_flag("force"),
            };
            to_json(&cx.peer.call::<p::ProjectInit>(params).await?)?
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
