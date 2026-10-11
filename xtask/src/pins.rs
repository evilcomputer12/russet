// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask pins`: generates `pins/stable.toml` from upstream release metadata and signs it with minisign (ARCHITECTURE §14.7) [D 87].
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W3 (Wave 1), W26 fills [`run`] and adds its flags to [`Args`].

/// Flags of `cargo xtask pins`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Arguments for the future implementation (ignored by the stub).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let _ = args.rest;
    Err(crate::not_implemented("pins", "pins.rs", "W3 (Wave 1), W26"))
}
