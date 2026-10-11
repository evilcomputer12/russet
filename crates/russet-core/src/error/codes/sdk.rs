// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `sdk` error codes (2100–2199). Owner: W3, W16b.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 4 (environment: run `russet doctor`).

use crate::error::ErrorCode;

/// 2100 `sdk.missing`: No usable Darwin SDK. Was E_SDK_MISSING.
pub const MISSING: ErrorCode = ErrorCode::new(2100, "sdk.missing", 4);
/// 2101 `sdk.incompatible`: The SDK's Xcode row does not match the Swift toolchain.
pub const INCOMPATIBLE: ErrorCode = ErrorCode::new(2101, "sdk.incompatible", 4);
/// 2102 `sdk.importFailed`: `sdk.import` failed.
pub const IMPORT_FAILED: ErrorCode = ErrorCode::new(2102, "sdk.importFailed", 4);
/// 2103 `sdk.incomplete`: The SDK misses part of the want-set (ARCHITECTURE §16.3). Was E_SDK_INCOMPLETE.
pub const INCOMPLETE: ErrorCode = ErrorCode::new(2103, "sdk.incomplete", 4);
/// 2104 `sdk.archiveInvalid`: A container is invalid; `details.layer` is xar, signature, pbzx, cpio, udif or hfs (a signature that does not chain to the pinned Apple Root CA, or a SHA-384 manifest-digest mismatch, is layer `signature`).
pub const ARCHIVE_INVALID: ErrorCode = ErrorCode::new(2104, "sdk.archiveInvalid", 4);
/// 2105 `sdk.pathTooLong`: An imported path exceeds the platform limit; `details {path, length, longPathsEnabled}`.
pub const PATH_TOO_LONG: ErrorCode = ErrorCode::new(2105, "sdk.pathTooLong", 4);
/// 2106 `sdk.linkFailed`: A junction or hardlink failed (for example on FAT/exFAT or network volumes, which are refused).
pub const LINK_FAILED: ErrorCode = ErrorCode::new(2106, "sdk.linkFailed", 4);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] =
    &[MISSING, INCOMPATIBLE, IMPORT_FAILED, INCOMPLETE, ARCHIVE_INVALID, PATH_TOO_LONG, LINK_FAILED];
