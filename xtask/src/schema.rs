// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask schema`: regenerates `schemas/rpc/**`, `extension/src/generated/**` and `docs/reference/rpc.md` from russet-protocol's registry (ARCHITECTURE §7.7).
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W2 fills [`run`] and adds its flags to [`Args`].

/// Flags of `cargo xtask schema`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Arguments for the future implementation (ignored by the stub).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let _ = args.rest;
    Err(crate::not_implemented("schema", "schema.rs", "W2"))
}
