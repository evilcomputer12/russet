// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Global flags (ARCHITECTURE §8.7): `--json --device --project -v/-q --color --no-daemon --yes`.
//!
//! W1 wrote the first version; W2 owns this file (CORE-07).

use std::path::PathBuf;

use clap::{Arg, ArgAction, ArgMatches, Command};

/// `--color`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

/// The parsed global flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalArgs {
    pub json: bool,
    pub device: Option<String>,
    pub project: Option<PathBuf>,
    pub verbose: u8,
    pub quiet: bool,
    pub color: ColorChoice,
    pub no_daemon: bool,
    pub yes: bool,
}

/// Adds the global flags to the root command.
pub fn args(cmd: Command) -> Command {
    cmd.arg(
        Arg::new("json")
            .long("json")
            .global(true)
            .action(ArgAction::SetTrue)
            .help("Machine output: one JSON document, or NDJSON for streams"),
    )
    .arg(Arg::new("device").long("device").global(true).value_name("UDID|ALIAS").help("The device to use"))
    .arg(Arg::new("project").long("project").global(true).value_name("DIR").help("The project root"))
    .arg(Arg::new("verbose").short('v').long("verbose").global(true).action(ArgAction::Count).help("More output"))
    .arg(Arg::new("quiet").short('q').long("quiet").global(true).action(ArgAction::SetTrue).help("Less output"))
    .arg(
        Arg::new("color")
            .long("color")
            .global(true)
            .value_name("WHEN")
            .value_parser(["auto", "always", "never"])
            .help("Colour output"),
    )
    .arg(
        Arg::new("no-daemon")
            .long("no-daemon")
            .global(true)
            .action(ArgAction::SetTrue)
            .help("Run the daemon in-process for this command"),
    )
    .arg(
        Arg::new("yes")
            .long("yes")
            .global(true)
            .action(ArgAction::SetTrue)
            .help("Pre-confirm plain confirmations (never typed ones)"),
    )
}

impl GlobalArgs {
    /// Reads the global flags from the root matches.
    pub fn from_matches(m: &ArgMatches) -> Self {
        let color = match m.get_one::<String>("color").map(String::as_str) {
            Some("always") => ColorChoice::Always,
            Some("never") => ColorChoice::Never,
            _ => ColorChoice::Auto,
        };
        Self {
            json: m.get_flag("json"),
            device: m.get_one::<String>("device").cloned(),
            project: m.get_one::<String>("project").map(PathBuf::from),
            verbose: m.get_count("verbose"),
            quiet: m.get_flag("quiet"),
            color,
            no_daemon: m.get_flag("no-daemon"),
            yes: m.get_flag("yes"),
        }
    }
}
