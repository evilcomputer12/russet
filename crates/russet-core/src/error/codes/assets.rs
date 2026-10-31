// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `assets` error codes (4100–4119). Owner: W21 (assets), W21b/W28b/W28c (storyboards).
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 7 (build failed).

use crate::error::ErrorCode;

/// 4100 `assets.invalid`: An asset catalog is invalid.
pub const INVALID: ErrorCode = ErrorCode::new(4100, "assets.invalid", 7);
/// 4101 `assets.storyboardUnsupported`: A storyboard or XIB element is unsupported (with file:line and the element; never partial output).
pub const STORYBOARD_UNSUPPORTED: ErrorCode = ErrorCode::new(4101, "assets.storyboardUnsupported", 7);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[INVALID, STORYBOARD_UNSUPPORTED];
