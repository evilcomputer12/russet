// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Daemon lifecycle: the single-instance lock, spawn, idle exit, shutdown and crash recovery (ARCHITECTURE §3.3).
//!
//! F0 stub (W1); W2 owns it.

use std::sync::Arc;

/// Daemon doctor checks (`russetd.json`, socket, live-device lock), for the doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
