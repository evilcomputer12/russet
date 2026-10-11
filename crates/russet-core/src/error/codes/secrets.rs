// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `secrets` error codes (1400–1499). Owner: W3.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 5 (credentials), except `secrets.notFound` (3).

use crate::error::ErrorCode;

/// 1400 `secrets.unavailable`: No secret backend is available. Was E_SECRETS_NO_BACKEND.
pub const UNAVAILABLE: ErrorCode = ErrorCode::new(1400, "secrets.unavailable", 5);
/// 1401 `secrets.locked`: The secret store is locked.
pub const LOCKED: ErrorCode = ErrorCode::new(1401, "secrets.locked", 5);
/// 1402 `secrets.notFound`: No secret with this id.
pub const NOT_FOUND: ErrorCode = ErrorCode::new(1402, "secrets.notFound", 3);
/// 1403 `secrets.passphraseRequired`: The passphrase store needs `RUSSET_SECRETS_PASSPHRASE` or a prompt.
pub const PASSPHRASE_REQUIRED: ErrorCode = ErrorCode::new(1403, "secrets.passphraseRequired", 5);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[UNAVAILABLE, LOCKED, NOT_FOUND, PASSPHRASE_REQUIRED];
