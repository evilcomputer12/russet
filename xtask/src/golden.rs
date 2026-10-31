// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask golden`: the golden corpus: normalize, compare, update (QA §3.4).
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W16 fills [`run`] and adds its flags to [`Args`].

/// Flags of `cargo xtask golden`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Arguments for the future implementation (ignored by the stub).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let _ = args.rest;
    Err(crate::not_implemented("golden", "golden.rs", "W16"))
}
