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
/// 5254 `gsa.codeRateLimited`: Apple refuses further two-factor codes for now (too many requests or attempts).
pub const CODE_RATE_LIMITED: ErrorCode = ErrorCode::new(5254, "gsa.codeRateLimited", 5);
/// 5255 `gsa.protocol`: GrandSlam or developerservices2 answered something Russet cannot read.
pub const PROTOCOL: ErrorCode = ErrorCode::new(5255, "gsa.protocol", 5);
/// 5256 `gsa.rootPinMismatch`: The Apple Root CA certificate does not match Russet's pinned SHA-256.
pub const ROOT_PIN_MISMATCH: ErrorCode = ErrorCode::new(5256, "gsa.rootPinMismatch", 5);
/// 5257 `gsa.sessionExpired`: The stored Apple ID session expired; sign in again.
pub const SESSION_EXPIRED: ErrorCode = ErrorCode::new(5257, "gsa.sessionExpired", 5);
/// 5258 `gsa.loginExpired`: The sign-in session is unknown or timed out (10 minutes); start again.
pub const LOGIN_EXPIRED: ErrorCode = ErrorCode::new(5258, "gsa.loginExpired", 5);
/// 5259 `gsa.ds2Failed`: developerservices2 refused a request (its `resultCode`).
pub const DS2_FAILED: ErrorCode = ErrorCode::new(5259, "gsa.ds2Failed", 5);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    BAD_CREDENTIALS,
    BAD_CODE,
    LOCKED,
    IDENTITY_REJECTED,
    CODE_RATE_LIMITED,
    PROTOCOL,
    ROOT_PIN_MISMATCH,
    SESSION_EXPIRED,
    LOGIN_EXPIRED,
    DS2_FAILED,
];
