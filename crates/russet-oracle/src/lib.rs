// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The Mac and GitHub Actions oracle runner (QA §3.4.5); Apple hardware only, read-only on the Mac.
//!
//! F0 stub (W1); W16 fills it.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");
