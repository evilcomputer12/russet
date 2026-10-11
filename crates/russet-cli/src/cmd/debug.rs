// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet debug` (ARCHITECTURE §8.7). Owner: W7 (Wave 1), W18 (jit-bench, Wave 3).
//!
//! F0 stub (W1): registered so `russet --help` lists it; running it reports `core.notImplemented` until
//! the owner fills [`command`] and [`run`].

use std::process::ExitCode;

use clap::{ArgMatches, Command};
use russet_core::Result;

use crate::CliCtx;

/// The `debug` command.
pub fn command() -> Command {
    super::stub(
        "debug",
        "Build, install, launch and debug with LLDB [--break FILE:LINE]… [--batch] [--detach]; debug prepare, jit-bench",
    )
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    super::not_implemented(m)
}
