// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet-adi-host`: the sandboxed helper that loads Apple's runtime-downloaded Android ADI libraries; no
//! network; JSON-lines IPC (ARCHITECTURE §3.1; SIGN §7.5). Layer L3 binary: F crates and russet-core only.
//!
//! F0 stub (W1); W22 fills it (SA-19).

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("russet-adi-host: not implemented yet (W22)");
    ExitCode::from(1)
}
