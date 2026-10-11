// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask lab`: runs the device-in-the-loop suites through the live daemon of its own `RUSSET_HOME`; refuses while a daemon of another `RUSSET_HOME` holds the live-device lock (ARCHITECTURE §3.3 item 7). Lane A only.
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W9 fills [`run`] and adds its flags to [`Args`].

/// Flags of `cargo xtask lab`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Arguments for the future implementation (ignored by the stub).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let _ = args.rest;
    Err(crate::not_implemented("lab", "lab.rs", "W9"))
}
