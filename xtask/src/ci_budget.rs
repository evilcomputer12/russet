// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask ci-budget`: reads the month's Actions usage through `gh api` and warns at 80 % of the included minutes (QA §4.8).
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W8 fills [`run`] and adds its flags to [`Args`].

/// Flags of `cargo xtask ci-budget`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Arguments for the future implementation (ignored by the stub).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let _ = args.rest;
    Err(crate::not_implemented("ci-budget", "ci_budget.rs", "W8"))
}
