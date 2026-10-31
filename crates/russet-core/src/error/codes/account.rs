// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `account` error codes (5100–5199). Owner: W6, W12.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 5 (credentials and accounts).

use crate::error::ErrorCode;

/// 5100 `account.ascUnauthorized`: App Store Connect rejected the token (401).
pub const ASC_UNAUTHORIZED: ErrorCode = ErrorCode::new(5100, "account.ascUnauthorized", 5);
/// 5101 `account.ascForbidden`: App Store Connect forbids the request (403).
pub const ASC_FORBIDDEN: ErrorCode = ErrorCode::new(5101, "account.ascForbidden", 5);
/// 5102 `account.ascScope`: The scoped token does not cover the request.
pub const ASC_SCOPE: ErrorCode = ErrorCode::new(5102, "account.ascScope", 5);
/// 5103 `account.ascRateLimited`: App Store Connect rate-limited the request.
pub const ASC_RATE_LIMITED: ErrorCode = ErrorCode::new(5103, "account.ascRateLimited", 5);
/// 5104 `account.individualKeyNoProvisioning`: Individual API keys cannot use the provisioning endpoints.
pub const INDIVIDUAL_KEY_NO_PROVISIONING: ErrorCode = ErrorCode::new(5104, "account.individualKeyNoProvisioning", 5);
/// 5105 `account.ascAmbiguous`: Several App Store Connect records match.
pub const ASC_AMBIGUOUS: ErrorCode = ErrorCode::new(5105, "account.ascAmbiguous", 5);
/// 5106 `account.twoFactorRequired`: Two-factor authentication is required.
pub const TWO_FACTOR_REQUIRED: ErrorCode = ErrorCode::new(5106, "account.twoFactorRequired", 5);
/// 5107 `account.appleServiceUnavailable`: An Apple service is unavailable.
pub const APPLE_SERVICE_UNAVAILABLE: ErrorCode = ErrorCode::new(5107, "account.appleServiceUnavailable", 5);
/// 5108 `account.insufficientRole`: The key's role is insufficient for this request.
pub const INSUFFICIENT_ROLE: ErrorCode = ErrorCode::new(5108, "account.insufficientRole", 5);
/// 5109 `account.keyInvalid`: The App Store Connect key is not a readable EC P-256 PKCS#8 (`.p8`) key.
pub const KEY_INVALID: ErrorCode = ErrorCode::new(5109, "account.keyInvalid", 5);
/// 5110 `account.noAccount`: No account is configured (or the requested one does not exist).
pub const NO_ACCOUNT: ErrorCode = ErrorCode::new(5110, "account.noAccount", 5);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    ASC_UNAUTHORIZED,
    ASC_FORBIDDEN,
    ASC_SCOPE,
    ASC_RATE_LIMITED,
    INDIVIDUAL_KEY_NO_PROVISIONING,
    ASC_AMBIGUOUS,
    TWO_FACTOR_REQUIRED,
    APPLE_SERVICE_UNAVAILABLE,
    INSUFFICIENT_ROLE,
    KEY_INVALID,
    NO_ACCOUNT,
];
