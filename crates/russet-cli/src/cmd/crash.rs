// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet crash` (ARCHITECTURE §8.7): list, show, fetch and symbolicate crash reports from the local store.
//! Owner: W11.

use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::crash::{
    CrashFetch, CrashFetchParams, CrashFormat, CrashList, CrashListParams, CrashSymbolicate, CrashSymbolicateParams,
    CrashSymbolicateResult,
};

use crate::CliCtx;

/// The `crash` command.
pub fn command() -> Command {
    Command::new("crash").about("Crash reports: list, show, fetch, symbolicate").subcommands([
        Command::new("list")
            .about("List stored crash reports (newest first)")
            .arg(Arg::new("bundle").long("bundle").value_name("ID").help("Only this bundle id"))
            .arg(Arg::new("since").long("since").value_name("TIME").help("Only crashes at or after this RFC 3339 time"))
            .arg(
                Arg::new("limit")
                    .long("limit")
                    .value_name("N")
                    .value_parser(clap::value_parser!(u32))
                    .help("At most N"),
            ),
        Command::new("show")
            .about("Show a crash report, symbolicated (text)")
            .arg(Arg::new("id").value_name("CRASH_ID").required(true).help("A crash id from `russet crash list`")),
        Command::new("symbolicate")
            .about("Symbolicate a stored crash or an .ips file")
            .arg(Arg::new("id").value_name("CRASH_ID").help("A stored crash id"))
            .arg(Arg::new("path").long("path").value_name("FILE").conflicts_with("id").help("An .ips file"))
            .arg(
                Arg::new("format")
                    .long("format")
                    .value_name("FMT")
                    .value_parser(["json", "text"])
                    .help("json (default) or text"),
            ),
        Command::new("fetch")
            .about("Pull new crash reports off the device into the store")
            .arg(Arg::new("bundle").long("bundle").value_name("ID").help("Only this bundle id"))
            .arg(Arg::new("since").long("since").value_name("TIME").help("Only crashes after this RFC 3339 time")),
    ])
}

fn json(v: &impl serde::Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| Error::new(codes::core::INTERNAL, format!("encoding the result: {e}")))
}

/// Runs this area's command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("crash", m)) = m.subcommand() else {
        return super::not_implemented(m);
    };
    match m.subcommand() {
        Some(("list", sub)) => list(sub, cx).await,
        Some(("show", sub)) => symbolicate(sub.get_one::<String>("id").map(String::as_str), None, true, cx).await,
        Some(("symbolicate", sub)) => {
            let text = sub.get_one::<String>("format").is_some_and(|f| f == "text");
            symbolicate(
                sub.get_one::<String>("id").map(String::as_str),
                sub.get_one::<String>("path").map(String::as_str),
                text,
                cx,
            )
            .await
        }
        Some(("fetch", sub)) => fetch(sub, cx).await,
        _ => super::not_implemented(m),
    }
}

async fn list(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let params = CrashListParams {
        bundle_id: m.get_one::<String>("bundle").cloned(),
        since: m.get_one::<String>("since").cloned(),
        limit: m.get_one::<u32>("limit").copied(),
    };
    let out = cx.peer.call::<CrashList>(params).await?;
    cx.reporter.result(&json(&out)?);
    Ok(ExitCode::SUCCESS)
}

async fn symbolicate(id: Option<&str>, path: Option<&str>, text: bool, cx: &CliCtx) -> Result<ExitCode> {
    let params = CrashSymbolicateParams {
        crash_id: id.map(str::to_owned),
        path: path.map(str::to_owned),
        format: Some(if text { CrashFormat::Text } else { CrashFormat::Json }),
    };
    let out = cx.peer.call::<CrashSymbolicate>(params).await?;
    match out {
        CrashSymbolicateResult::Text { crash } => cx.reporter.result(&serde_json::json!({ "crash": crash })),
        CrashSymbolicateResult::Json { crash } => cx.reporter.result(&json(&crash)?),
    }
    Ok(ExitCode::SUCCESS)
}

async fn fetch(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let params = CrashFetchParams {
        udid: cx.global.device.clone(),
        bundle_id: m.get_one::<String>("bundle").cloned(),
        since: m.get_one::<String>("since").cloned(),
    };
    let out = cx.peer.call::<CrashFetch>(params).await?;
    cx.reporter.result(&json(&out)?);
    Ok(ExitCode::SUCCESS)
}
