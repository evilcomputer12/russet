// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Storyboard and XIB reader, launch-screen converter (B12), NIBArchive codec and the storyboard/XIB compiler
//! (B21a–c) [D 75, D 90].
//!
//! Layer F. F0 stub (W1); W21 owns the crate in Wave 3, W21b in Wave 4, W28b and W28c in Wave 6.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

/// Storyboard/XIB → `.storyboardc`/`.nib` compilation (pre-created for W21b).
pub mod compile;
/// The NIBArchive codec (pre-created for W21b).
pub mod nib;
