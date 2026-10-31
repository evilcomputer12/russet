// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet symbols` (ARCHITECTURE §8.7): the device-symbol cache — status, plan, consent-gated fetch, prune.
//! Owner: W11.
//!
//! A fetch needs the planned total passed as `--yes-bytes N` (the N3 consent gate, RD §11.2); the download
//! itself needs owner approval (Q13) and tier GREEN (DC-09).

use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::debug::{
    DebugSymbolsFetch, DebugSymbolsFetchParams, DebugSymbolsPlan, DebugSymbolsPlanParams, DebugSymbolsPrune,
    DebugSymbolsPruneParams, DebugSymbolsStatus, DebugSymbolsStatusParams, SymbolTier,
};

use crate::CliCtx;

/// The `symbols` command group.
pub fn command() -> Command {
    let tier = || Arg::new("tier").long("tier").value_name("TIER").value_parser(["core", "full"]).help("core or full");
    Command::new("symbols").about("Device symbols: status, plan, fetch, prune").subcommands([
        Command::new("status").about("What the local symbol cache holds for the device's build"),
        Command::new("plan")
            .about("The files and sizes a fetch would download, with the space/quota check")
            .arg(tier()),
        Command::new("fetch")
            .about("Fetch device symbols (needs the planned total via --yes-bytes; owner Q13, tier GREEN)")
            .arg(tier().required(true))
            .arg(
                Arg::new("yes-bytes")
                    .long("yes-bytes")
                    .value_name("N")
                    .value_parser(clap::value_parser!(u64))
                    .help("Confirm the planned total in bytes (from `russet symbols plan`)"),
            )
            .arg(
                Arg::new("keep-raw-dsc")
                    .long("keep-raw-dsc")
                    .action(clap::ArgAction::SetTrue)
                    .help("Keep the raw dyld shared cache after extraction"),
            ),
        Command::new("prune")
            .about("Prune least-recently-used symbol sets")
            .arg(
                Arg::new("max-bytes")
                    .long("max-bytes")
                    .value_name("N")
                    .value_parser(clap::value_parser!(u64))
                    .help("Prune down to at most N bytes (default: the quota)"),
            )
            .arg(Arg::new("build").long("build").value_name("BUILD").help("Prune only this build's set")),
    ])
}

fn json(v: &impl serde::Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| Error::new(codes::core::INTERNAL, format!("encoding the result: {e}")))
}

fn tier_of(m: &ArgMatches) -> Option<SymbolTier> {
    m.get_one::<String>("tier").map(|t| if t == "full" { SymbolTier::Full } else { SymbolTier::Core })
}

/// Runs this area's command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("symbols", m)) = m.subcommand() else {
        return super::not_implemented(m);
    };
    match m.subcommand() {
        Some(("status", _)) => {
            let out =
                cx.peer.call::<DebugSymbolsStatus>(DebugSymbolsStatusParams { udid: cx.global.device.clone() }).await?;
            cx.reporter.result(&json(&out)?);
            Ok(ExitCode::SUCCESS)
        }
        Some(("plan", sub)) => {
            let out = cx
                .peer
                .call::<DebugSymbolsPlan>(DebugSymbolsPlanParams { udid: cx.global.device.clone(), tier: tier_of(sub) })
                .await?;
            cx.reporter.result(&json(&out)?);
            Ok(ExitCode::SUCCESS)
        }
        Some(("fetch", sub)) => {
            let params = DebugSymbolsFetchParams {
                udid: cx.global.device.clone(),
                tier: tier_of(sub).unwrap_or(SymbolTier::Core),
                confirm_bytes: sub.get_one::<u64>("yes-bytes").copied(),
                keep_raw_dsc: sub.get_flag("keep-raw-dsc").then_some(true),
            };
            let out = cx.peer.call::<DebugSymbolsFetch>(params).await?;
            cx.reporter.result(&json(&out)?);
            Ok(ExitCode::SUCCESS)
        }
        Some(("prune", sub)) => {
            let params = DebugSymbolsPruneParams {
                max_bytes: sub.get_one::<u64>("max-bytes").copied(),
                build: sub.get_one::<String>("build").cloned(),
            };
            let out = cx.peer.call::<DebugSymbolsPrune>(params).await?;
            cx.reporter.result(&json(&out)?);
            Ok(ExitCode::SUCCESS)
        }
        _ => super::not_implemented(m),
    }
}
