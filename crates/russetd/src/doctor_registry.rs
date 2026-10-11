// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The doctor seam (ARCHITECTURE §5.3, §6.1): every domain crate exports
//! `pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>>`, and this fixed list concatenates them.
//!
//! Seam file: lead-only after W1 merges (PLAN §0 rule 2). Layer-L2 crates may not depend on russet-doctor
//! (ARCHITECTURE §5.2): the host checks live in russet-doctor itself, and the configuration, secrets and toolchain
//! checks in `handlers/doctor.rs` (W3).

use std::sync::Arc;

use russet_doctor::{Check, Registry};

/// A crate's `checks()` function.
type ChecksFn = fn() -> Vec<Arc<dyn Check>>;

/// Every check, in report order.
pub fn all_checks() -> Vec<Arc<dyn Check>> {
    let sources: &[ChecksFn] = &[
        russet_doctor::checks,           // host (W3)
        crate::daemon::checks,           // daemon (W2)
        crate::handlers::doctor::checks, // config, secrets, toolchain (W3)
        russet_device::checks,           // usbmux, pairing, Developer Mode, DDI (W4, W10)
        russet_sdk::checks,              // SDK (W5, W16b)
        russet_build::checks,            // build (W5)
        russet_sign::checks,             // signing (W6)
        russet_asc::checks,              // App Store Connect (W6)
        russet_account::checks,          // accounts (W6, W12)
        russet_gsa::checks,              // free Apple ID (W22)
        russet_anisette::checks,         // anisette (W22)
        russet_debug::checks,            // LLDB and libpython (W7)
        russet_ship::checks,             // publishing (W23)
        russet_xctest::checks,           // XCTest (W32)
        russet_mcp::checks,              // MCP registrations (W13)
    ];
    sources.iter().flat_map(|source| source()).collect()
}

/// A registry with every check registered.
pub fn registry() -> Registry {
    let mut r = Registry::new();
    for c in all_checks() {
        r.register(c);
    }
    r
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn check_ids_are_unique() {
        let ids = registry().ids();
        let unique: BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "duplicate doctor check ids: {ids:?}");
    }
}
