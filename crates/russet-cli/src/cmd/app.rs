// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet app`, `russet install`, `russet launch` (ARCHITECTURE §8.7). Owner: W4.
//!
//! `russet app list|install|uninstall|terminate|launch`; `russet install <app|ipa>` and `russet launch <bundle>` are
//! the same as `app install` and `app launch`. `app launch` prints `{pid, executable, leaseId}` and returns; the
//! app keeps running (the lease only carries its console and exit notifications).

use std::collections::BTreeMap;
use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::app as p;

use crate::CliCtx;

fn launch_args(c: Command) -> Command {
    c.arg(Arg::new("bundle").required(true).value_name("BUNDLE_ID").help("The installed app's bundle id"))
        .arg(Arg::new("suspended").long("suspended").action(ArgAction::SetTrue).help("Start stopped before main"))
        .arg(
            Arg::new("kill-existing")
                .long("kill-existing")
                .action(ArgAction::SetTrue)
                .help("Terminate a running instance first"),
        )
        .arg(
            Arg::new("console")
                .long("console")
                .action(ArgAction::SetTrue)
                .help("Attach the app's stdio (read it with `russet logs --pid N`)"),
        )
        .arg(
            Arg::new("env")
                .long("env")
                .value_name("K=V")
                .action(ArgAction::Append)
                .help("An environment variable (repeatable)"),
        )
        .arg(Arg::new("args").num_args(0..).last(true).value_name("APP_ARGS").help("Arguments for the app (after --)"))
}

fn install_args(c: Command) -> Command {
    c.arg(Arg::new("path").required(true).value_name("APP_OR_IPA").help("A signed .app directory or .ipa"))
}

/// This area's top-level commands, as a [`super::group`].
pub fn command() -> Command {
    let app = Command::new("app")
        .about("Apps on the device: list, install, uninstall, terminate, launch")
        .subcommand_required(true)
        .subcommand(
            Command::new("list")
                .about("List installed apps")
                .arg(Arg::new("all").long("all").action(ArgAction::SetTrue).help("Include system apps"))
                .arg(
                    Arg::new("prefix")
                        .long("prefix")
                        .value_name("BUNDLE_PREFIX")
                        .help("Only bundle ids with this prefix"),
                ),
        )
        .subcommand(install_args(Command::new("install").about("Install an .app or .ipa")))
        .subcommand(
            Command::new("uninstall")
                .about("Uninstall an app")
                .arg(Arg::new("bundle").required(true).value_name("BUNDLE_ID")),
        )
        .subcommand(
            Command::new("terminate")
                .about("Terminate an app (SIGKILL), by bundle id or pid")
                .arg(Arg::new("bundle").value_name("BUNDLE_ID").required_unless_present("pid"))
                .arg(Arg::new("pid").long("pid").value_name("PID").value_parser(clap::value_parser!(u32))),
        )
        .subcommand(launch_args(Command::new("launch").about("Launch an installed app")));
    super::group(
        "app",
        [
            app,
            install_args(Command::new("install").about("Install an .app or .ipa on the device")),
            launch_args(Command::new("launch").about("Launch an installed app by bundle id")),
        ],
    )
}

/// `K=V` pairs.
pub fn parse_env(m: &ArgMatches) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for kv in m.get_many::<String>("env").into_iter().flatten() {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| Error::new(codes::core::INVALID_ARGUMENT, format!("--env {kv:?}: expected KEY=VALUE")))?;
        if k.is_empty() {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, format!("--env {kv:?}: empty key")));
        }
        env.insert(k.to_owned(), v.to_owned());
    }
    Ok(env)
}

/// The `app.launch` params of a launch command line.
pub fn launch_params(m: &ArgMatches, udid: Option<String>) -> Result<p::AppLaunchParams> {
    Ok(p::AppLaunchParams {
        udid,
        bundle_id: m.get_one::<String>("bundle").cloned().unwrap_or_default(),
        args: m.get_many::<String>("args").into_iter().flatten().cloned().collect(),
        env: parse_env(m)?,
        suspended: Some(m.get_flag("suspended")),
        kill_existing: Some(m.get_flag("kill-existing")),
        console: Some(m.get_flag("console")),
    })
}

fn print<T: serde::Serialize>(cx: &CliCtx, v: &T) -> Result<ExitCode> {
    let v = serde_json::to_value(v)
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot render the result: {e}")))?;
    cx.reporter.result(&v);
    Ok(ExitCode::SUCCESS)
}

async fn install(cx: &CliCtx, m: &ArgMatches, udid: Option<String>) -> Result<ExitCode> {
    let path = super::device::absolute(m.get_one::<String>("path").map(String::as_str).unwrap_or_default());
    print(cx, &cx.peer.call::<p::AppInstall>(p::AppInstallParams { udid, path }).await?)
}

async fn launch(cx: &CliCtx, m: &ArgMatches, udid: Option<String>) -> Result<ExitCode> {
    print(cx, &cx.peer.call::<p::AppLaunch>(launch_params(m, udid)?).await?)
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let udid = cx.global.device.clone();
    match m.subcommand() {
        Some(("install", sm)) => install(cx, sm, udid).await,
        Some(("launch", sm)) => launch(cx, sm, udid).await,
        Some(("app", am)) => match am.subcommand() {
            Some(("list", sm)) => {
                let params = p::AppListParams {
                    udid,
                    kind: sm.get_flag("all").then_some(p::AppKind::All),
                    bundle_id_prefix: sm.get_one::<String>("prefix").cloned(),
                };
                print(cx, &cx.peer.call::<p::AppList>(params).await?)
            }
            Some(("install", sm)) => install(cx, sm, udid).await,
            Some(("launch", sm)) => launch(cx, sm, udid).await,
            Some(("uninstall", sm)) => {
                let bundle_id = sm.get_one::<String>("bundle").cloned().unwrap_or_default();
                print(cx, &cx.peer.call::<p::AppUninstall>(p::AppUninstallParams { udid, bundle_id }).await?)
            }
            Some(("terminate", sm)) => {
                let params = p::AppTerminateParams {
                    udid,
                    pid: sm.get_one::<u32>("pid").copied(),
                    bundle_id: sm.get_one::<String>("bundle").cloned(),
                };
                print(cx, &cx.peer.call::<p::AppTerminate>(params).await?)
            }
            _ => super::not_implemented(m),
        },
        _ => super::not_implemented(m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> ArgMatches {
        let mut argv = vec!["russet"];
        argv.extend_from_slice(args);
        crate::root_command().try_get_matches_from(argv).expect("valid command line")
    }

    #[test]
    fn launch_lines_become_params() {
        let m = parse(&[
            "app",
            "launch",
            "dev.russet.test.probe",
            "--suspended",
            "--env",
            "A=1",
            "--env",
            "B=x=y",
            "--",
            "-flag",
            "v",
        ]);
        let (_, am) = m.subcommand().expect("app");
        let (_, lm) = am.subcommand().expect("launch");
        let params = launch_params(lm, Some("iphone12".into())).expect("params");
        assert_eq!(params.bundle_id, "dev.russet.test.probe");
        assert_eq!(params.suspended, Some(true));
        assert_eq!(params.kill_existing, Some(false));
        assert_eq!(params.env.get("B").map(String::as_str), Some("x=y"));
        assert_eq!(params.args, vec!["-flag".to_owned(), "v".to_owned()]);
        // The top-level `launch` is the same command.
        let m = parse(&["launch", "dev.russet.test.probe", "--console"]);
        let (name, lm) = m.subcommand().expect("launch");
        assert_eq!(name, "launch");
        assert_eq!(launch_params(lm, None).expect("params").console, Some(true));
        assert!(crate::root_command().try_get_matches_from(["russet", "app", "terminate"]).is_err());
    }

    #[test]
    fn bad_env_pairs_are_rejected() {
        let m = parse(&["launch", "b", "--env", "NOEQUALS"]);
        let (_, lm) = m.subcommand().expect("launch");
        assert_eq!(parse_env(lm).expect_err("rejected").code(), codes::core::INVALID_ARGUMENT);
    }
}
