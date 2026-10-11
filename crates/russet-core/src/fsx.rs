// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Filesystem helpers (core §5.1).

use std::path::Path;

/// Writes `b` to a temporary file in the same directory, fsyncs it and renames it over `p`.
pub fn atomic_write(p: &Path, b: &[u8]) -> crate::Result<()> {
    todo!("W2 (CORE-02)")
}

/// Fails with `core.diskFull` (and a hint) unless `needed + reserve` bytes are free on `p`'s filesystem.
pub fn ensure_space(p: &Path, needed: u64, reserve: u64) -> crate::Result<()> {
    todo!("W2 (CORE-02)")
}

/// Creates `p` 0700 and checks that the current user owns it (Unix), or sets a user-only ACL (Windows).
pub fn ensure_private_dir(p: &Path) -> crate::Result<()> {
    todo!("W2 (CORE-02)")
}
