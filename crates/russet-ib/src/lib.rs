// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Storyboard and XIB reader, launch-screen converter (B12), NIBArchive codec and the storyboard/XIB compiler
//! (B21a–c) [D 75, D 90].
//!
//! Layer F (MIT). F0 stub (W1); W21 owns the crate in Wave 3, W21b in Wave 4, W28b and W28c in Wave 6.

/// Storyboard/XIB → `.storyboardc`/`.nib` compilation (pre-created for W21b).
pub mod compile;
/// The NIBArchive codec (pre-created for W21b).
pub mod nib;
