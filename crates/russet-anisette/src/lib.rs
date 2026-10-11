// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Anisette providers (local ADI through russet-adi-host, RemoteV3) and runtime acquisition of Apple's Android
//! libraries, with consent and hash pins; never redistributed (SA-19).
//!
//! F0 stub (W1); W22 fills it, W27c owns `windows`. ARCHITECTURE §5.3 names no cross-area signature for this crate.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::Arc;

/// The Windows ADI host shims (pre-created for W27c).
#[cfg(windows)]
pub mod windows;

/// Anisette doctor checks, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
