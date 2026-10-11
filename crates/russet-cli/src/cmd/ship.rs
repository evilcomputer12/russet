// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet ship` (ARCHITECTURE §8.7). Owner: W12 (archive, ipa; Wave 2), W23 (the rest; Wave 4).
//!
//! F0 stub (W1): registered so `russet --help` lists it; running it reports `core.notImplemented` until
//! the owner fills [`command`] and [`run`].

use std::process::ExitCode;

use clap::{ArgMatches, Command};
use russet_core::Result;

use crate::CliCtx;

/// The `ship` command.
pub fn command() -> Command {
    super::stub(
        "ship",
        "Publishing: archive, ipa, stamp, validate, upload, builds, testflight, metadata, version, submit, review, ci, crashes, compare",
    )
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    super::not_implemented(m)
}
