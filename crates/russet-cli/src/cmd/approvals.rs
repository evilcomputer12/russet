// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet approvals`, `russet approve`, `russet deny` (ARCHITECTURE §8.7). Owner: W2.
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
        "approvals",
        [
            super::stub("approvals", "Approvals: list"),
            super::stub("approve", "Grant an approval (TTY only)"),
            super::stub("deny", "Deny an approval (TTY only) [--reason …]"),
        ],
    )
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    super::not_implemented(m)
}
