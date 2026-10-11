// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask ci`, `cargo xtask test` and `cargo xtask build`: the CI gates locally (`--profile pr|full`) and the
//! cargo wrappers that run disk-guard first, with `CARGO_INCREMENTAL=0` (PLAN §0 rules 7 and 10) [D 81].
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W8 fills [`run`], [`run_test`] and [`run_build`] and adds their flags.

/// Flags of `cargo xtask ci`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Arguments for the future implementation (ignored by the stub).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

/// Flags of `cargo xtask test`.
#[derive(Debug, clap::Args)]
pub struct TestArgs {
    /// Arguments for the future implementation (ignored by the stub).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

/// Flags of `cargo xtask build`.
#[derive(Debug, clap::Args)]
pub struct BuildArgs {
    /// Arguments for the future implementation (ignored by the stub).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let _ = args.rest;
    Err(crate::not_implemented("ci", "ci.rs", "W8"))
}

pub fn run_test(args: TestArgs) -> anyhow::Result<()> {
    let _ = args.rest;
    Err(crate::not_implemented("test", "ci.rs", "W8"))
}

pub fn run_build(args: BuildArgs) -> anyhow::Result<()> {
    let _ = args.rest;
    Err(crate::not_implemented("build", "ci.rs", "W8"))
}
