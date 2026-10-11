// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet run`, `russet sessions`, `russet stop`, `russet relaunch`, `russet reload` (ARCHITECTURE §8.7). Owner: W7 (run, sessions, stop), W11 (relaunch, reload).
//!
//! F0 stub (W1): registered so `russet --help` lists it; running it reports `core.notImplemented` until
//! the owner fills [`command`] and [`run`].

use std::process::ExitCode;

use clap::{ArgMatches, Command};
use russet_core::Result;

use crate::CliCtx;

/// This area's top-level commands, as a [`super::group`].
pub fn command() -> Command {
    super::group(
        "run",
        [
            super::stub("run", "Build, install and launch the app [--detach | --background] [-- app args]"),
            super::stub("sessions", "List run, debug and Flutter sessions"),
            super::stub("stop", "Stop a session [<session> | --all]"),
            super::stub("relaunch", "Quick Relaunch a Swift session"),
            super::stub("reload", "Hot reload a session [--restart]"),
        ],
    )
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    super::not_implemented(m)
}
