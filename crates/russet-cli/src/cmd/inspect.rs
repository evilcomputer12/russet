// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet inspect`, `russet lint` (ARCHITECTURE §8.7). Owner: W5 (Wave 1), W16 (lint, Wave 2).
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
        "inspect",
        [
            super::stub("inspect", "Inspect an .app or .ipa: Info.plist, Mach-O, lints"),
            super::stub("lint", "Lint an .app or .ipa [--store]"),
        ],
    )
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    super::not_implemented(m)
}
