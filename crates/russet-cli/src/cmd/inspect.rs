// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet inspect`, `russet lint` (ARCHITECTURE §8.7). Owner: W5 (Wave 1), W16 (lint, Wave 2).
//!
//! ```text
//! russet inspect <App.app|App.ipa>     build.inspect: Info.plist, DT keys, Mach-O summaries, structural lints
//! russet lint <path> [--store]          W16 (B05, B18)
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command, value_parser};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::build as p;

use super::build::absolute;
use crate::CliCtx;

/// This area's top-level commands, as a [`super::group`].
pub fn command() -> Command {
    super::group(
        "inspect",
        [
            Command::new("inspect").about("Inspect an .app or .ipa: Info.plist, DT keys, Mach-O, lints").arg(
                Arg::new("path")
                    .required(true)
                    .value_name("PATH")
                    .value_parser(value_parser!(PathBuf))
                    .help("An .app directory or an .ipa file"),
            ),
            super::stub("lint", "Lint an .app or .ipa [--store]"),
        ],
    )
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    match m.subcommand() {
        Some(("inspect", sub)) => {
            let path = absolute(sub.get_one::<PathBuf>("path").map(PathBuf::as_path))?;
            let report = cx.peer.call::<p::BuildInspect>(p::BuildInspectParams { path }).await?;
            let value = serde_json::to_value(&report)
                .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot render the result: {e}")))?;
            cx.reporter.result(&value);
            Ok(ExitCode::SUCCESS)
        }
        _ => super::not_implemented(m),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn inspect_needs_a_path() {
        assert!(crate::root_command().try_get_matches_from(["russet", "inspect"]).is_err());
        assert!(crate::root_command().try_get_matches_from(["russet", "inspect", "A.ipa"]).is_ok());
    }
}
