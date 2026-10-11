// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `perf` error codes (8100–8199). Owner: W10.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! ARCHITECTURE §13.4 lists no exit code for this family; W1 uses 6 (device state), like the device plane it belongs to.

use crate::error::ErrorCode;

/// 8100 `perf.serviceUnavailable`: The performance service is unavailable.
pub const SERVICE_UNAVAILABLE: ErrorCode = ErrorCode::new(8100, "perf.serviceUnavailable", 6);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[SERVICE_UNAVAILABLE];
