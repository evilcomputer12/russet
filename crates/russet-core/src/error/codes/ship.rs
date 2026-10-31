// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `ship` error codes (7000–7199). Owner: W12, W23.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 10 (publishing failed).

use crate::error::ErrorCode;

/// 7000 `ship.noAppRecord`: No App Store Connect app record for the bundle id.
pub const NO_APP_RECORD: ErrorCode = ErrorCode::new(7000, "ship.noAppRecord", 10);
/// 7001 `ship.duplicateBuild`: The build number was already uploaded.
pub const DUPLICATE_BUILD: ErrorCode = ErrorCode::new(7001, "ship.duplicateBuild", 10);
/// 7002 `ship.uploadFailed`: The upload failed; `details.itms`.
pub const UPLOAD_FAILED: ErrorCode = ErrorCode::new(7002, "ship.uploadFailed", 10);
/// 7003 `ship.commitAmbiguous`: The upload commit state is ambiguous.
pub const COMMIT_AMBIGUOUS: ErrorCode = ErrorCode::new(7003, "ship.commitAmbiguous", 10);
/// 7004 `ship.stampNoConsent`: `xcode-compat` metadata needs the recorded consent (ARCHITECTURE §16.4).
pub const STAMP_NO_CONSENT: ErrorCode = ErrorCode::new(7004, "ship.stampNoConsent", 10);
/// 7005 `ship.stampNoSdkMetadata`: The SDK metadata needed for stamping is missing.
pub const STAMP_NO_SDK_METADATA: ErrorCode = ErrorCode::new(7005, "ship.stampNoSdkMetadata", 10);
/// 7006 `ship.validationFailed`: Validation failed.
pub const VALIDATION_FAILED: ErrorCode = ErrorCode::new(7006, "ship.validationFailed", 10);
/// 7007 `ship.versionStateInvalid`: The App Store version is not editable.
pub const VERSION_STATE_INVALID: ErrorCode = ErrorCode::new(7007, "ship.versionStateInvalid", 10);
/// 7008 `ship.reviewSubmitFailed`: The review submission failed; Apple's errors verbatim in `details.errors`.
pub const REVIEW_SUBMIT_FAILED: ErrorCode = ErrorCode::new(7008, "ship.reviewSubmitFailed", 10);
/// 7009 `ship.ciCloudSigningForbidden`: "Cloud signing permission error": cloud signing needs an Admin-role team key.
pub const CI_CLOUD_SIGNING_FORBIDDEN: ErrorCode = ErrorCode::new(7009, "ship.ciCloudSigningForbidden", 10);
/// 7010 `ship.ciRunnerUnavailable`: The pinned runner label or Xcode version is missing from the image.
pub const CI_RUNNER_UNAVAILABLE: ErrorCode = ErrorCode::new(7010, "ship.ciRunnerUnavailable", 10);
/// 7011 `ship.archiveFailed`: The archive (or its dSYM generation) could not be created.
pub const ARCHIVE_FAILED: ErrorCode = ErrorCode::new(7011, "ship.archiveFailed", 10);
/// 7012 `ship.ipaInvalid`: The input to the IPA writer is not a usable `.app` or `.xcarchive`.
pub const IPA_INVALID: ErrorCode = ErrorCode::new(7012, "ship.ipaInvalid", 10);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    NO_APP_RECORD,
    DUPLICATE_BUILD,
    UPLOAD_FAILED,
    COMMIT_AMBIGUOUS,
    STAMP_NO_CONSENT,
    STAMP_NO_SDK_METADATA,
    VALIDATION_FAILED,
    VERSION_STATE_INVALID,
    REVIEW_SUBMIT_FAILED,
    CI_CLOUD_SIGNING_FORBIDDEN,
    CI_RUNNER_UNAVAILABLE,
    ARCHIVE_FAILED,
    IPA_INVALID,
];
