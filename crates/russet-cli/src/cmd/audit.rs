// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet audit tail [--kinds …] [--client C] [--limit N] [--follow]` (ARCHITECTURE §8.7, §6.5). Owner: W2.

use std::collections::BTreeSet;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::error::{Error, Result};
use russet_protocol::audit::{AuditEntry, AuditKind, AuditTail, AuditTailParams};

use crate::CliCtx;

/// How often `--follow` asks for new entries.
const FOLLOW_EVERY: Duration = Duration::from_secs(1);

/// The `audit` command.
pub fn command() -> Command {
    Command::new("audit")
        .about("The audit log: tail [--kinds …] [--client C] [--limit N] [--follow]")
        .subcommand_required(true)
        .subcommand(
            Command::new("tail")
                .about("The newest audit entries")
                .arg(
                    Arg::new("kinds")
                        .long("kinds")
                        .value_name("KINDS")
                        .value_delimiter(',')
                        .value_parser(["rpc", "approval", "account-mutation"])
                        .help("Only these kinds"),
                )
                .arg(Arg::new("client").long("client").value_name("NAME").help("Only this client"))
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u32))
                        .help("How many (50)"),
                )
                .arg(
                    Arg::new("follow")
                        .short('f')
                        .long("follow")
                        .action(ArgAction::SetTrue)
                        .help("Keep printing new entries"),
                ),
        )
}

/// One entry as a line.
pub fn human_entry(e: &AuditEntry) -> String {
    let tier = e.tier.map_or_else(String::new, |t| format!(" tier {t}"));
    format!(
        "{}  {:?}  {} {}  {}{}  {}  {} ms",
        e.ts,
        e.kind,
        e.client.name,
        format!("{:?}", e.client.kind).to_lowercase(),
        e.method,
        tier,
        e.outcome,
        e.duration_ms
    )
}

fn key(e: &AuditEntry) -> String {
    format!("{}|{}|{}|{}", e.ts, e.method, e.params_digest, e.outcome)
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some((_, m)) = m.subcommand() else { return Err(Error::invalid_argument("no audit command")) };
    let Some(("tail", a)) = m.subcommand() else { return Err(Error::invalid_argument("unknown audit command")) };
    let kinds: Vec<AuditKind> = a
        .get_many::<String>("kinds")
        .into_iter()
        .flatten()
        .filter_map(|k| match k.as_str() {
            "rpc" => Some(AuditKind::Rpc),
            "approval" => Some(AuditKind::Approval),
            "account-mutation" => Some(AuditKind::AccountMutation),
            _ => None,
        })
        .collect();
    let params = AuditTailParams {
        limit: a.get_one::<u32>("limit").copied(),
        kinds,
        client: a.get_one::<String>("client").cloned(),
    };
    let first = cx.peer.call::<AuditTail>(params.clone()).await?;
    if !a.get_flag("follow") {
        cx.output(&first, |r| r.entries.iter().map(human_entry).collect::<Vec<_>>().join("\n"))?;
        return Ok(ExitCode::SUCCESS);
    }
    // Follow: NDJSON, one entry per line, polling for new ones.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut emit = |entries: Vec<AuditEntry>| {
        for e in entries {
            if seen.insert(key(&e)) {
                cx.reporter.item(&serde_json::to_value(&e).unwrap_or_default(), &human_entry(&e));
            }
        }
    };
    emit(first.entries);
    loop {
        tokio::select! {
            () = tokio::time::sleep(FOLLOW_EVERY) => {}
            _ = tokio::signal::ctrl_c() => break,
        }
        let next = cx.peer.call::<AuditTail>(AuditTailParams { limit: Some(200), ..params.clone() }).await?;
        emit(next.entries);
    }
    Ok(ExitCode::SUCCESS)
}
