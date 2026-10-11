// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! XML and binary plist I/O, the OpenStep reader and writer, and Xcode build-setting macro expansion
//! (build B01). Layer F: permissive, and no `russet-*` dependency except other F crates (ARCHITECTURE §5.2).
//!
//! F0 stub (W1); W5 fills it. ARCHITECTURE §5.3 names no cross-area signature for this crate.

pub use plist;
