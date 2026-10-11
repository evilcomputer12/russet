// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Archive, dSYMs, the store IPA, validation, metadata stamping, upload, TestFlight, App Store versions and review, and the Apple-hardware mode (SIGN §9).
//!
//! F0 stub (W1); W12 (Wave 2), W23 (Wave 4) fills it. ARCHITECTURE §5.3 names no cross-area signature for this crate.

use std::sync::Arc;

/// Doctor checks of this crate, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
