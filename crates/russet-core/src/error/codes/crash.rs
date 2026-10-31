// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `crash` error codes (6100–6149). Owner: W11.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 9 (launch or debug failed), except `crash.notFound` (3).

use crate::error::ErrorCode;

/// 6100 `crash.notFound`: No crash report with this id.
pub const NOT_FOUND: ErrorCode = ErrorCode::new(6100, "crash.notFound", 3);
/// 6101 `crash.symbolicationFailed`: Symbolication failed.
pub const SYMBOLICATION_FAILED: ErrorCode = ErrorCode::new(6101, "crash.symbolicationFailed", 9);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[NOT_FOUND, SYMBOLICATION_FAILED];
