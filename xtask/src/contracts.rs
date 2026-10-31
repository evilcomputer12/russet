// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask contracts`: contract tests: schema and CLI snapshots, the DAP script, MCP conformance in both eras (QA §3.7).
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W15 fills [`run`] and adds its flags to [`Args`].

/// Flags of `cargo xtask contracts`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Arguments for the future implementation (ignored by the stub).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let _ = args.rest;
    Err(crate::not_implemented("contracts", "contracts.rs", "W15"))
}
