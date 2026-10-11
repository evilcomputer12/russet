// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Writes MCP registrations for the agent clients (plan, apply, detect; IDE §6.6).
//!
//! F0 stub (W1); W13 fills it (MCP-04). ARCHITECTURE §5.3 names no cross-area signature for this crate.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");
