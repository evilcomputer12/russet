// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! GrandSlam authentication, 2FA and DS2 for the opt-in free Apple ID (SA-18).
//!
//! F0 stub (W1); W22 fills it. ARCHITECTURE §5.3 names no cross-area signature for this crate.

use std::sync::Arc;

/// Doctor checks of this crate, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
