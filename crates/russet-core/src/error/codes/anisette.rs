// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `anisette` error codes (5270–5299). Owner: W22.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 5 (credentials and accounts).

use crate::error::ErrorCode;

/// 5270 `anisette.libsMissing`: The anisette libraries are not downloaded.
pub const LIBS_MISSING: ErrorCode = ErrorCode::new(5270, "anisette.libsMissing", 5);
/// 5271 `anisette.pinMismatch`: The anisette libraries do not match their hash pins.
pub const PIN_MISMATCH: ErrorCode = ErrorCode::new(5271, "anisette.pinMismatch", 5);
/// 5272 `anisette.adiFailed`: The ADI helper failed.
pub const ADI_FAILED: ErrorCode = ErrorCode::new(5272, "anisette.adiFailed", 5);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[LIBS_MISSING, PIN_MISMATCH, ADI_FAILED];
