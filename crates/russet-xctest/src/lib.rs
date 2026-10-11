// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! XCTest and XCUITest runs on the device through idevice `xctest` (testmanagerd), with JUnit output [D 80].
//!
//! F0 stub (W1); W32 (Wave 4), W28 (Wave 6) fills it. ARCHITECTURE §5.3 names no cross-area signature for this crate.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::Arc;

/// Doctor checks of this crate, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
