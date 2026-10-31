// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! GrandSlam authentication, 2FA and DS2 for the opt-in free Apple ID (SA-18).
//!
//! F0 stub (W1); W22 fills it. ARCHITECTURE §5.3 names no cross-area signature for this crate.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::Arc;

/// Doctor checks of this crate, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
