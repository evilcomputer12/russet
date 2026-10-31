// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `.xcassets` model, icon expansion, partial Info.plist and asset symbols (build B10).
//!
//! Layer F. F0 stub (W1); W21 fills it. ARCHITECTURE §5.3 names no cross-area signature for this crate.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");
