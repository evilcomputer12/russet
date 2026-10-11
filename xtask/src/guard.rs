// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask guard <check>…`: repository guards (QA-02; ARCHITECTURE §19): `apple-files` (alias
//! `check-no-apple`), `secrets`, `reuse`, `typos`, `public-terms`, `all`; `--artifacts DIR` scans upload
//! directories too.
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W8 fills it. W1's stub already routes `guard apple-files` to the
//! [`crate::no_apple`] scan, so the alias holds from day one.

use std::path::{Path, PathBuf};

use crate::no_apple::Finding;

/// Flags of `cargo xtask guard`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Guards to run: apple-files, secrets, reuse, typos, public-terms, all
    #[arg(value_name = "CHECK")]
    pub checks: Vec<String>,
    /// Extra directories to scan (caches, upload artifacts)
    #[arg(long = "artifacts", value_name = "DIR")]
    pub artifacts: Vec<PathBuf>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    if !args.checks.is_empty() && args.checks.iter().all(|c| c == "apple-files") {
        return crate::no_apple::run(crate::no_apple::Args { paths: args.artifacts });
    }
    Err(crate::not_implemented("guard", "guard.rs", "W8"))
}

/// Whether a provenance allowlist admits an Apple-format file (ARCHITECTURE §19 [D 88]): only under the roots
/// compiled in here (`tests/fixtures/xip/`, `fuzz/russet-xar/corpus/`), only when that root's
/// `PROVENANCE.toml` lists the file with a matching SHA-256 and a committed generator script. The F0 stub admits
/// nothing; W8 implements it.
pub fn provenance_allows(root: &Path, finding: &Finding) -> bool {
    let _ = (root, finding);
    false
}
