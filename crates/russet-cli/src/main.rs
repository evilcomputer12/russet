// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `russet` binary. Multi-call dispatch (`russet` or `russetd` as `argv[0]`) lives in [`russet_cli::main_entry`].

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

fn main() -> std::process::ExitCode {
    russet_cli::main_entry()
}
