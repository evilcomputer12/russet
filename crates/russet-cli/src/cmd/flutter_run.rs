// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The run half of `russet flutter` (`run`, `attach`, `device run-debug`, `device forward`). Owner: W18.
//!
//! Not a top-level area: W17's `cmd/flutter.rs` dispatcher calls it. Pre-created by W1 (PLAN §3 [D 86]).

use std::process::ExitCode;

use clap::{ArgMatches, Command};
use russet_core::Result;

use crate::CliCtx;

/// Subcommands this module adds under `russet flutter`.
pub fn subcommands() -> Vec<Command> {
    Vec::new()
}

/// Runs one of [`subcommands`]; `m` is the `flutter` command's matches.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    super::not_implemented(m)
}
