// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Host-platform specifics of the build plane.
//!
//! F0 stub (W1); W5 owns this file, W27b the `windows` module.

/// The Windows build plane: junction view, `.exe` shims, response files, short paths (pre-created for W27b).
#[cfg(windows)]
pub mod windows;
