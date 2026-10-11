// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet device` (ARCHITECTURE §8.7). Owner: W4 (Wave 1), W10 (Wave 2).
//!
//! `russet device list|info|readiness|prepare|pair|reserve|alias` and `russet device ddi status|mount`; `unpair` and
//! `ddi list` arrive with W10. The global `--device <udid|alias>` names the device; it may be omitted when exactly
//! one is attached. Every command prints the method's result (`--json`: one JSON document).

use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::device as p;

use crate::CliCtx;

/// The `device` command.
pub fn command() -> Command {
    Command::new("device")
        .about("Devices: list, info, readiness, prepare, pair, reserve, alias; ddi status|mount")
        .subcommand_required(true)
        // `--udid` overrides the global `--device` for one command (hidden: `--device` is the documented flag).
        .arg(Arg::new("udid").long("udid").value_name("UDID").global(true).hide(true))
        .subcommand(Command::new("list").about("List attached devices"))
        .subcommand(Command::new("info").about("Show a device's identity and properties"))
        .subcommand(
            Command::new("readiness")
                .about("Pairing, Developer Mode, DDI, lock and tunnel state")
                .arg(Arg::new("refresh").long("refresh").action(ArgAction::SetTrue).help("Re-read everything")),
        )
        .subcommand(
            Command::new("prepare")
                .about("Pair if needed and mount the Developer Disk Image when missing")
                .arg(Arg::new("dir").long("dir").value_name("DIR").help("Mount from this DDI directory"))
                .arg(
                    Arg::new("reveal-developer-mode")
                        .long("reveal-developer-mode")
                        .action(ArgAction::SetTrue)
                        .help("Reveal the Developer Mode toggle in Settings when it is off"),
                ),
        )
        .subcommand(
            Command::new("pair").about("Pair with the device (unlock it and tap Trust)").arg(
                Arg::new("host-name").long("host-name").value_name("NAME").help("The name the Trust dialog shows"),
            ),
        )
        .subcommand(Command::new("unpair").about("Remove the pairing (Wave 2)"))
        .subcommand(Command::new("reserve").about("Reserve the device for this client until interrupted").arg(
            Arg::new("purpose").long("purpose").value_name("TEXT").help("Shown to clients that get device.reserved"),
        ))
        .subcommand(
            Command::new("alias")
                .about("Name a device in [devices.aliases] (the --device UDID, or the only one attached)")
                .arg(Arg::new("alias").required(true).value_name("ALIAS"))
                .arg(Arg::new("udid").long("udid").value_name("UDID").help("The UDID (default: the selected device)"))
                .arg(Arg::new("remove").long("remove").action(ArgAction::SetTrue).help("Remove the alias")),
        )
        .subcommand(
            Command::new("ddi")
                .about("Developer Disk Image: status, mount (list arrives with Wave 2)")
                .subcommand_required(true)
                .subcommand(Command::new("status").about("Show the mounted DDI and where it came from"))
                .subcommand(Command::new("list").about("List imported DDIs (Wave 2)"))
                .subcommand(
                    Command::new("mount")
                        .about("Mount a DDI (default: the [device.ddi] order)")
                        .arg(
                            Arg::new("dir").long("dir").value_name("DIR").help("A DDI root (Restore/ + version.plist)"),
                        )
                        .arg(
                            Arg::new("imported")
                                .long("imported")
                                .value_name("DDI_BUILD")
                                .num_args(0..=1)
                                .help("An imported DDI, the newest when no build is given (Wave 2)"),
                        )
                        .arg(Arg::new("force").long("force").action(ArgAction::SetTrue).help("Replace a mounted DDI")),
                ),
        )
}

/// The `ddiSource` of `--dir D` or `--imported [BUILD]`.
pub fn ddi_source(m: &ArgMatches) -> Option<p::DdiSourceParam> {
    if let Some(dir) = m.get_one::<String>("dir") {
        return Some(p::DdiSourceParam::Dir { path: absolute(dir) });
    }
    if m.try_contains_id("imported").unwrap_or(false) && m.contains_id("imported") {
        return Some(p::DdiSourceParam::Imported { ddi_build: m.get_one::<String>("imported").cloned() });
    }
    None
}

/// The hidden per-command `--udid`, wherever it was given below `device`.
fn selected(m: &ArgMatches) -> Option<String> {
    let mut cur = m;
    let mut found = cur.get_one::<String>("udid").cloned();
    while let Some((_, sub)) = cur.subcommand() {
        if let Ok(Some(u)) = sub.try_get_one::<String>("udid") {
            found = Some(u.clone());
        }
        cur = sub;
    }
    found
}

/// Paths go on the wire absolute (ARCHITECTURE §7.3).
pub fn absolute(path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return path.to_owned();
    }
    std::env::current_dir().map(|d| d.join(p).display().to_string()).unwrap_or_else(|_| path.to_owned())
}

fn wave2(what: &str) -> Error {
    Error::new(codes::core::NOT_IMPLEMENTED, format!("`russet {what}` arrives with Wave 2 (W10)"))
        .with_hint("See docs/PLAN.md §5.1.")
}

fn print<T: serde::Serialize>(cx: &CliCtx, v: &T) -> Result<ExitCode> {
    let v = serde_json::to_value(v)
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot render the result: {e}")))?;
    cx.reporter.result(&v);
    Ok(ExitCode::SUCCESS)
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("device", m)) = m.subcommand() else {
        return super::not_implemented(m);
    };
    let udid = selected(m).or_else(|| cx.global.device.clone());
    match m.subcommand() {
        Some(("list", _)) => print(cx, &cx.peer.call::<p::DeviceList>(p::DeviceListParams {}).await?),
        Some(("info", _)) => print(cx, &cx.peer.call::<p::DeviceInfo>(p::DeviceInfoParams { udid }).await?),
        Some(("readiness", sm)) => {
            let refresh = sm.get_flag("refresh").then_some(true);
            print(cx, &cx.peer.call::<p::DeviceReadiness>(p::DeviceReadinessParams { udid, refresh }).await?)
        }
        Some(("prepare", sm)) => {
            let params = p::DevicePrepareParams {
                udid,
                ddi_source: ddi_source(sm),
                reveal_developer_mode: sm.get_flag("reveal-developer-mode").then_some(true),
            };
            print(cx, &cx.peer.call::<p::DevicePrepare>(params).await?)
        }
        Some(("pair", sm)) => {
            let params = p::DevicePairParams { udid, host_name: sm.get_one::<String>("host-name").cloned() };
            let r = cx.peer.call::<p::DevicePair>(params).await?;
            let code = if r.status == p::PairStatus::Paired { ExitCode::SUCCESS } else { ExitCode::from(6) };
            print(cx, &r)?;
            Ok(code)
        }
        Some(("unpair", _)) => Err(wave2("device unpair")),
        Some(("reserve", sm)) => {
            let purpose = sm.get_one::<String>("purpose").cloned().unwrap_or_else(|| "russet device reserve".into());
            let r = cx.peer.call::<p::DeviceReserve>(p::DeviceReserveParams { udid, purpose }).await?;
            print(cx, &r)?;
            // The reservation belongs to this connection: hold it until interrupted or the daemon goes away.
            cx.peer.closed().await;
            Ok(ExitCode::SUCCESS)
        }
        Some(("alias", sm)) => {
            let alias = sm.get_one::<String>("alias").cloned().unwrap_or_default();
            let target = if sm.get_flag("remove") {
                String::new()
            } else {
                match sm.get_one::<String>("udid").cloned().or(udid.clone()) {
                    Some(u) => u,
                    None => {
                        // The only attached device.
                        let list = cx.peer.call::<p::DeviceList>(p::DeviceListParams {}).await?;
                        match list.devices.as_slice() {
                            [one] => one.udid.clone(),
                            [] => return Err(Error::new(codes::device::NOT_FOUND, "no iPhone is attached")),
                            _ => {
                                return Err(Error::new(codes::core::INVALID_ARGUMENT, "several devices are attached")
                                    .with_hint("Pass --udid <UDID>."));
                            }
                        }
                    }
                }
            };
            print(cx, &cx.peer.call::<p::DeviceAliasSet>(p::DeviceAliasSetParams { udid: target, alias }).await?)
        }
        Some(("ddi", dm)) => match dm.subcommand() {
            Some(("status", _)) => {
                print(cx, &cx.peer.call::<p::DeviceDdiStatus>(p::DeviceDdiStatusParams { udid }).await?)
            }
            Some(("list", _)) => Err(wave2("device ddi list")),
            Some(("mount", sm)) => {
                let params = p::DeviceDdiMountParams {
                    udid,
                    source: ddi_source(sm),
                    force: sm.get_flag("force").then_some(true),
                };
                print(cx, &cx.peer.call::<p::DeviceDdiMount>(params).await?)
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
    fn the_device_tree_parses() {
        crate::root_command().debug_assert();
        let m = parse(&["device", "ddi", "mount", "--dir", "/srv/ddi/27A266a/iOS", "--force"]);
        let (_, dm) = m.subcommand().expect("device");
        let (_, ddi) = dm.subcommand().expect("ddi");
        let (_, mount) = ddi.subcommand().expect("mount");
        assert_eq!(ddi_source(mount), Some(p::DdiSourceParam::Dir { path: "/srv/ddi/27A266a/iOS".into() }));
        assert!(mount.get_flag("force"));
        let m = parse(&["device", "ddi", "mount", "--imported"]);
        let mount =
            m.subcommand().and_then(|(_, d)| d.subcommand()).and_then(|(_, d)| d.subcommand()).expect("mount").1;
        assert_eq!(ddi_source(mount), Some(p::DdiSourceParam::Imported { ddi_build: None }));
        let m = parse(&["--json", "--device", "iphone12", "device", "readiness", "--refresh"]);
        assert_eq!(crate::global::GlobalArgs::from_matches(&m).device.as_deref(), Some("iphone12"));
    }

    #[test]
    fn relative_paths_become_absolute() {
        assert!(std::path::Path::new(&absolute("ddi/iOS")).is_absolute());
        assert_eq!(absolute("/x"), "/x");
    }
}
