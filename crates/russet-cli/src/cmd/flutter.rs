// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet flutter` (ARCHITECTURE §8.7). Owner: W17 (the dispatcher), W18 (`flutter_run.rs`), W24 (gen-snapshot).
//!
//! F0 stub (W1): registered so `russet --help` lists it; running it reports `core.notImplemented` until
//! the owner fills [`command`] and [`run`].

use std::process::ExitCode;

use clap::{ArgMatches, Command};
use russet_core::Result;

use crate::CliCtx;

/// The `flutter` command.
pub fn command() -> Command {
    super::stub("flutter", "Flutter: register, build, run; device …; gen-snapshot ensure|list")
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    super::not_implemented(m)
}
