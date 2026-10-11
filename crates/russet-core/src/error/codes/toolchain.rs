// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `toolchain` error codes (2000–2099). Owner: W3.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 4 (environment: run `russet doctor`).

use crate::error::ErrorCode;

/// 2000 `toolchain.missing`: A required tool is not installed.
pub const MISSING: ErrorCode = ErrorCode::new(2000, "toolchain.missing", 4);
/// 2001 `toolchain.versionMismatch`: The tool version does not match the pins. Was E_TOOLCHAIN_MISMATCH.
pub const VERSION_MISMATCH: ErrorCode = ErrorCode::new(2001, "toolchain.versionMismatch", 4);
/// 2002 `toolchain.broken`: The tool fails to start (for example a missing libpython).
pub const BROKEN: ErrorCode = ErrorCode::new(2002, "toolchain.broken", 4);
/// 2003 `toolchain.downloadFailed`: A toolchain download failed.
pub const DOWNLOAD_FAILED: ErrorCode = ErrorCode::new(2003, "toolchain.downloadFailed", 4);
/// 2004 `toolchain.verifyFailed`: A download failed its sha256, minisign or PGP check.
pub const VERIFY_FAILED: ErrorCode = ErrorCode::new(2004, "toolchain.verifyFailed", 4);
/// 2005 `toolchain.pinsInvalid`: The pin manifest is invalid or its signature does not verify.
pub const PINS_INVALID: ErrorCode = ErrorCode::new(2005, "toolchain.pinsInvalid", 4);
/// 2006 `toolchain.linkerUnsupported`: The linker is not darwin-tools >= 1.1.0 or LLD >= 23.1.3. Was E_LINKER_UNSUPPORTED.
pub const LINKER_UNSUPPORTED: ErrorCode = ErrorCode::new(2006, "toolchain.linkerUnsupported", 4);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] =
    &[MISSING, VERSION_MISMATCH, BROKEN, DOWNLOAD_FAILED, VERIFY_FAILED, PINS_INVALID, LINKER_UNSUPPORTED];
