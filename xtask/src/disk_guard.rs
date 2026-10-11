// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask disk-guard`: measures free space on /home against the tiers RED < 8 GiB, AMBER 8–25, GREEN 25–45, BLUE >= 45 with a 20 GiB reserve (QA §10; PLAN §0 rule 6).
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W8 fills [`run`] and adds its flags to [`Args`].

/// Flags of `cargo xtask disk-guard`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Arguments for the future implementation (ignored by the stub).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let _ = args.rest;
    Err(crate::not_implemented("disk-guard", "disk_guard.rs", "W8"))
}
