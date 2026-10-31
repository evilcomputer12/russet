// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `sign` error codes (5000–5099). Owner: W6.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 8 (signing or provisioning failed).

use crate::error::ErrorCode;

/// 5000 `sign.noIdentity`: No usable signing identity.
pub const NO_IDENTITY: ErrorCode = ErrorCode::new(5000, "sign.noIdentity", 8);
/// 5001 `sign.profileMismatch`: No provisioning profile matches the bundle, team, certificate and device.
pub const PROFILE_MISMATCH: ErrorCode = ErrorCode::new(5001, "sign.profileMismatch", 8);
/// 5002 `sign.entitlementNotInProfile`: An entitlement is not granted by the profile.
pub const ENTITLEMENT_NOT_IN_PROFILE: ErrorCode = ErrorCode::new(5002, "sign.entitlementNotInProfile", 8);
/// 5003 `sign.entitlementValueNotAllowed`: An entitlement value is not allowed by the profile.
pub const ENTITLEMENT_VALUE_NOT_ALLOWED: ErrorCode = ErrorCode::new(5003, "sign.entitlementValueNotAllowed", 8);
/// 5004 `sign.entitlementUnresolvedVariable`: An entitlement contains an unresolved `$(VARIABLE)`.
pub const ENTITLEMENT_UNRESOLVED_VARIABLE: ErrorCode = ErrorCode::new(5004, "sign.entitlementUnresolvedVariable", 8);
/// 5005 `sign.lintFailed`: A signing lint failed.
pub const LINT_FAILED: ErrorCode = ErrorCode::new(5005, "sign.lintFailed", 8);
/// 5006 `sign.codesignFailed`: Code signing failed.
pub const CODESIGN_FAILED: ErrorCode = ErrorCode::new(5006, "sign.codesignFailed", 8);
/// 5007 `sign.profileInvalid`: A provisioning profile cannot be parsed (not CMS, or not a profile plist).
pub const PROFILE_INVALID: ErrorCode = ErrorCode::new(5007, "sign.profileInvalid", 8);
/// 5008 `sign.bundleInvalid`: The path is not a signable bundle (no Info.plist, identifier or executable).
pub const BUNDLE_INVALID: ErrorCode = ErrorCode::new(5008, "sign.bundleInvalid", 8);
/// 5009 `sign.verifyFailed`: A signature does not verify (code pages, sealed resources, CMS or profile rules).
pub const VERIFY_FAILED: ErrorCode = ErrorCode::new(5009, "sign.verifyFailed", 8);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    NO_IDENTITY,
    PROFILE_MISMATCH,
    ENTITLEMENT_NOT_IN_PROFILE,
    ENTITLEMENT_VALUE_NOT_ALLOWED,
    ENTITLEMENT_UNRESOLVED_VARIABLE,
    LINT_FAILED,
    CODESIGN_FAILED,
    PROFILE_INVALID,
    BUNDLE_INVALID,
    VERIFY_FAILED,
];
