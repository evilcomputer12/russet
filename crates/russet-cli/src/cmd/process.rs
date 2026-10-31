// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet process` (ARCHITECTURE §8.7). Owner: W4 (Wave 1), W10 (Wave 2).
//!
//! `russet process list` and `russet process signal <pid> <signal>`; a signal is a number or a name (`SIGCONT`,
//! `CONT`, `kill`, …).

use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::process as p;

use crate::CliCtx;

/// The `process` command.
pub fn command() -> Command {
    Command::new("process")
        .about("Device processes: list, signal")
        .subcommand_required(true)
        .subcommand(Command::new("list").about("List the device's processes"))
        .subcommand(
            Command::new("signal")
                .about("Send a POSIX signal (SIGCONT resumes a suspended launch)")
                .arg(Arg::new("pid").required(true).value_parser(clap::value_parser!(u32)))
                .arg(
                    Arg::new("signal")
                        .required(true)
                        .value_name("SIGNAL")
                        .help("A number or a name (SIGCONT, KILL, …)"),
                ),
        )
}

/// Darwin signal numbers by name.
pub fn parse_signal(s: &str) -> Result<i32> {
    if let Ok(n) = s.parse::<i32>() {
        return Ok(n);
    }
    let name = s.trim().to_ascii_uppercase();
    let name = name.strip_prefix("SIG").unwrap_or(&name);
    let n = match name {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "ILL" => 4,
        "TRAP" => 5,
        "ABRT" => 6,
        "KILL" => 9,
        "BUS" => 10,
        "SEGV" => 11,
        "TERM" => 15,
        "STOP" => 17,
        "TSTP" => 18,
        "CONT" => 19,
        "USR1" => 30,
        "USR2" => 31,
        _ => {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, format!("unknown signal {s:?}"))
                .with_hint("Use a number, or a name such as SIGCONT, SIGKILL, SIGTERM or SIGABRT."));
        }
    };
    Ok(n)
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("process", pm)) = m.subcommand() else {
        return super::not_implemented(m);
    };
    let udid = cx.global.device.clone();
    let value = match pm.subcommand() {
        Some(("list", _)) => serde_json::to_value(cx.peer.call::<p::ProcessList>(p::ProcessListParams { udid }).await?),
        Some(("signal", sm)) => {
            let pid = sm.get_one::<u32>("pid").copied().unwrap_or_default();
            let signal = parse_signal(sm.get_one::<String>("signal").map(String::as_str).unwrap_or_default())?;
            serde_json::to_value(cx.peer.call::<p::ProcessSignal>(p::ProcessSignalParams { udid, pid, signal }).await?)
        }
        _ => return super::not_implemented(m),
    }
    .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot render the result: {e}")))?;
    cx.reporter.result(&value);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_parse_by_number_and_darwin_name() {
        assert_eq!(parse_signal("19").expect("n"), 19);
        assert_eq!(parse_signal("SIGCONT").expect("name"), 19);
        assert_eq!(parse_signal("kill").expect("name"), 9);
        assert_eq!(parse_signal("SIGUSR1").expect("darwin"), 30);
        assert!(parse_signal("SIGNOPE").is_err());
    }
}
