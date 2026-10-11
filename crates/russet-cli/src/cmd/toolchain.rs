// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet toolchain list|resolve|install|uninstall|gc|pins update` (ARCHITECTURE §8.7, §16). Owner: W3
//! (`pins update`: W26).
//!
//! `install` takes a managed component (`python-shared`, `darwin-tools` in Wave 1) and an optional version (the
//! pinned one by default); it is long-running and cancellable with Ctrl-C.

use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::toolchain as p;

use super::config::{project_param, report};
use crate::CliCtx;

const COMPONENTS: [&str; 10] =
    ["swift", "darwin-sdk", "linker", "lldb", "lldb-dap", "libpython", "xtool", "flutter", "gen-snapshot-ios", "llvm"];
const MANAGED: [&str; 6] = ["python-shared", "darwin-tools", "swift", "llvm", "flutter", "xtool"];

/// The `toolchain` command.
pub fn command() -> Command {
    Command::new("toolchain")
        .about("Toolchains: list, resolve, install, uninstall, gc, pins update")
        .subcommand_required(true)
        .subcommand(
            Command::new("list")
                .about("Every candidate of every component, with versions, provenance, env and problems")
                .arg(Arg::new("refresh").long("refresh").action(ArgAction::SetTrue).help("Re-run every probe")),
        )
        .subcommand(
            Command::new("resolve")
                .about("The tool a build or debug session would use, with its environment")
                .arg(Arg::new("component").value_name("COMPONENT").required(true).value_parser(COMPONENTS)),
        )
        .subcommand(
            Command::new("install")
                .about("Download, verify and install a pinned component (python-shared, darwin-tools)")
                .arg(Arg::new("component").value_name("COMPONENT").required(true).value_parser(MANAGED))
                .arg(Arg::new("version").value_name("VERSION").help("Default: the pinned version")),
        )
        .subcommand(
            Command::new("uninstall")
                .about("Remove an installed version")
                .arg(Arg::new("component").value_name("COMPONENT").required(true).value_parser(MANAGED))
                .arg(Arg::new("version").value_name("VERSION").required(true)),
        )
        .subcommand(
            Command::new("gc")
                .about("Remove installed versions that are not selected")
                .arg(Arg::new("dry-run").long("dry-run").action(ArgAction::SetTrue)),
        )
        .subcommand(
            Command::new("pins").about("The pin manifest (W26)").subcommand_required(true).subcommand(
                Command::new("update")
                    .about("Fetch and verify a newer pin manifest (W26)")
                    .arg(Arg::new("args").num_args(0..).trailing_var_arg(true).allow_hyphen_values(true).hide(true)),
            ),
        )
}

fn parse<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).map_err(|_| {
        Error::new(codes::core::INVALID_ARGUMENT, format!("`{s}` is not a component"))
            .with_hint("Run `russet toolchain --help`.")
    })
}

/// One `russet toolchain` invocation, as RPC params.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ToolchainRequest {
    List(p::ToolchainListParams),
    Resolve(p::ToolchainResolveParams),
    Install(p::ToolchainInstallParams),
    Uninstall(p::ToolchainUninstallParams),
    Gc(p::ToolchainGcParams),
    PinsUpdate,
}

/// Reads the invocation from the root matches.
pub(crate) fn request(m: &ArgMatches, project: Option<String>) -> Result<ToolchainRequest> {
    let Some(("toolchain", m)) = m.subcommand() else {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, "not a toolchain command")
            .with_hint("Run `russet toolchain --help`."));
    };
    let component = |a: &ArgMatches| a.get_one::<String>("component").cloned().unwrap_or_default();
    Ok(match m.subcommand() {
        Some(("list", a)) => {
            ToolchainRequest::List(p::ToolchainListParams { project, refresh: a.get_flag("refresh").then_some(true) })
        }
        Some(("resolve", a)) => {
            ToolchainRequest::Resolve(p::ToolchainResolveParams { component: parse(&component(a))?, project })
        }
        Some(("install", a)) => ToolchainRequest::Install(p::ToolchainInstallParams {
            component: parse(&component(a))?,
            version: a.get_one::<String>("version").cloned(),
        }),
        Some(("uninstall", a)) => ToolchainRequest::Uninstall(p::ToolchainUninstallParams {
            component: parse(&component(a))?,
            version: a.get_one::<String>("version").cloned().unwrap_or_default(),
        }),
        Some(("gc", a)) => {
            ToolchainRequest::Gc(p::ToolchainGcParams { dry_run: a.get_flag("dry-run").then_some(true) })
        }
        Some(("pins", _)) => ToolchainRequest::PinsUpdate,
        _ => {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "unknown toolchain subcommand")
                .with_hint("Run `russet toolchain --help`."));
        }
    })
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    match request(m, project_param(&cx.global)?)? {
        ToolchainRequest::List(params) => report(cx, &cx.peer.call::<p::ToolchainList>(params).await?)?,
        ToolchainRequest::Resolve(params) => report(cx, &cx.peer.call::<p::ToolchainResolve>(params).await?)?,
        ToolchainRequest::Install(params) => report(cx, &cx.peer.call::<p::ToolchainInstall>(params).await?)?,
        ToolchainRequest::Uninstall(params) => report(cx, &cx.peer.call::<p::ToolchainUninstall>(params).await?)?,
        ToolchainRequest::Gc(params) => report(cx, &cx.peer.call::<p::ToolchainGc>(params).await?)?,
        ToolchainRequest::PinsUpdate => return super::not_implemented(m),
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> ToolchainRequest {
        let m = crate::root_command().try_get_matches_from(args).expect("parses");
        request(&m, None).expect("request")
    }

    #[test]
    fn invocations_become_params() {
        assert_eq!(
            parse_args(&["russet", "--json", "toolchain", "list"]),
            ToolchainRequest::List(p::ToolchainListParams { project: None, refresh: None })
        );
        assert_eq!(
            parse_args(&["russet", "toolchain", "install", "python-shared"]),
            ToolchainRequest::Install(p::ToolchainInstallParams {
                component: p::ManagedComponent::PythonShared,
                version: None
            })
        );
        assert_eq!(
            parse_args(&["russet", "toolchain", "install", "darwin-tools", "1.1.0"]),
            ToolchainRequest::Install(p::ToolchainInstallParams {
                component: p::ManagedComponent::DarwinTools,
                version: Some("1.1.0".into())
            })
        );
        assert_eq!(
            parse_args(&["russet", "toolchain", "resolve", "lldb-dap"]),
            ToolchainRequest::Resolve(p::ToolchainResolveParams {
                component: p::ToolComponent::LldbDap,
                project: None
            })
        );
        assert_eq!(
            parse_args(&["russet", "toolchain", "gc", "--dry-run"]),
            ToolchainRequest::Gc(p::ToolchainGcParams { dry_run: Some(true) })
        );
        assert_eq!(parse_args(&["russet", "toolchain", "pins", "update"]), ToolchainRequest::PinsUpdate);
        assert!(crate::root_command().try_get_matches_from(["russet", "toolchain", "install", "gcc"]).is_err());
    }
}
