// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `config` error codes (1300–1399). Owner: W3.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 4 (environment: run `russet doctor`).

use crate::error::ErrorCode;

/// 1300 `config.invalid`: A configuration file is invalid (with file:line:col). Was E_MANIFEST_INVALID.
pub const INVALID: ErrorCode = ErrorCode::new(1300, "config.invalid", 4);
/// 1301 `config.projectNotFound`: No `russet.toml` or supported project was found. Was E_PROJECT_NOT_FOUND.
pub const PROJECT_NOT_FOUND: ErrorCode = ErrorCode::new(1301, "config.projectNotFound", 4);
/// 1302 `config.unknownKey`: An unknown TOML key (with file:line:col).
pub const UNKNOWN_KEY: ErrorCode = ErrorCode::new(1302, "config.unknownKey", 4);
/// 1303 `config.readOnly`: The setting cannot be changed in this scope.
pub const READ_ONLY: ErrorCode = ErrorCode::new(1303, "config.readOnly", 4);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[INVALID, PROJECT_NOT_FOUND, UNKNOWN_KEY, READ_ONLY];
