// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet tap`, `russet swipe`, `russet button`, `russet type` (ARCHITECTURE §8.7). Owner: W10.
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
        "input",
        [
            super::stub("tap", "Tap the device screen"),
            super::stub("swipe", "Swipe on the device screen"),
            super::stub("button", "Press a hardware button"),
            super::stub("type", "Type text on the device"),
        ],
    )
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    super::not_implemented(m)
}
