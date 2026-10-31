// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `flutter` error codes (6150–6199). Owner: W18.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 9 (launch or debug failed).

use crate::error::ErrorCode;

/// 6150 `flutter.vmServiceNotFound`: The Dart VM service URI was not found.
pub const VM_SERVICE_NOT_FOUND: ErrorCode = ErrorCode::new(6150, "flutter.vmServiceNotFound", 9);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[VM_SERVICE_NOT_FOUND];
