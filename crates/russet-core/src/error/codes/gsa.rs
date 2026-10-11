// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `gsa` error codes (5250–5269). Owner: W22.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 5 (credentials and accounts).

use crate::error::ErrorCode;

/// 5250 `gsa.badCredentials`: The Apple ID or password is wrong.
pub const BAD_CREDENTIALS: ErrorCode = ErrorCode::new(5250, "gsa.badCredentials", 5);
/// 5251 `gsa.badCode`: The two-factor code is wrong.
pub const BAD_CODE: ErrorCode = ErrorCode::new(5251, "gsa.badCode", 5);
/// 5252 `gsa.locked`: The Apple ID is locked.
pub const LOCKED: ErrorCode = ErrorCode::new(5252, "gsa.locked", 5);
/// 5253 `gsa.identityRejected`: Apple rejected the device identity (anisette).
pub const IDENTITY_REJECTED: ErrorCode = ErrorCode::new(5253, "gsa.identityRejected", 5);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[BAD_CREDENTIALS, BAD_CODE, LOCKED, IDENTITY_REJECTED];
