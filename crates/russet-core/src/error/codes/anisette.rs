// SPDX-License-Identifier: AGPL-3.0-only
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
/// 5273 `anisette.consentRequired`: Downloading Apple's ADI libraries needs the user's consent.
pub const CONSENT_REQUIRED: ErrorCode = ErrorCode::new(5273, "anisette.consentRequired", 5);
/// 5274 `anisette.fetchFailed`: The range download of the ADI libraries failed.
pub const FETCH_FAILED: ErrorCode = ErrorCode::new(5274, "anisette.fetchFailed", 5);
/// 5275 `anisette.remoteFailed`: The anisette-v3 server failed or answered something unreadable.
pub const REMOTE_FAILED: ErrorCode = ErrorCode::new(5275, "anisette.remoteFailed", 5);
/// 5276 `anisette.unsupported`: Local ADI does not run on this host.
pub const UNSUPPORTED: ErrorCode = ErrorCode::new(5276, "anisette.unsupported", 5);
/// 5277 `anisette.provisioningFailed`: Apple refused to provision this machine for anisette.
pub const PROVISIONING_FAILED: ErrorCode = ErrorCode::new(5277, "anisette.provisioningFailed", 5);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    LIBS_MISSING,
    PIN_MISMATCH,
    ADI_FAILED,
    CONSENT_REQUIRED,
    FETCH_FAILED,
    REMOTE_FAILED,
    UNSUPPORTED,
    PROVISIONING_FAILED,
];
