// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `xcode` error codes (4150–4199). Owner: W28.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 7 (build failed).

use crate::error::ErrorCode;

/// 4150 `xcode.projectUnsupported`: The Xcode project uses an unsupported feature.
pub const PROJECT_UNSUPPORTED: ErrorCode = ErrorCode::new(4150, "xcode.projectUnsupported", 7);
/// 4151 `xcode.pifFailed`: PIF generation failed.
pub const PIF_FAILED: ErrorCode = ErrorCode::new(4151, "xcode.pifFailed", 7);
/// 4152 `xcode.swbFailed`: `russet-swb` (Swift Build) failed.
pub const SWB_FAILED: ErrorCode = ErrorCode::new(4152, "xcode.swbFailed", 7);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[PROJECT_UNSUPPORTED, PIF_FAILED, SWB_FAILED];
