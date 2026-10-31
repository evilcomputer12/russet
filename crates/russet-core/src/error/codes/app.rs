// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `app` error codes (3100–3199). Owner: W4.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 6 (device state), except `app.launchFailed` (9).

use crate::error::ErrorCode;

/// 3100 `app.notInstalled`: The app is not installed (`app.not_installed`).
pub const NOT_INSTALLED: ErrorCode = ErrorCode::new(3100, "app.notInstalled", 6);
/// 3101 `app.installFailed`: Installation failed; `details.subkind`: signature, no-matching-profile, minimum-os, app-id-mismatch (`install.failed`).
pub const INSTALL_FAILED: ErrorCode = ErrorCode::new(3101, "app.installFailed", 6);
/// 3102 `app.launchFailed`: The app failed to launch (`launch.failed`).
pub const LAUNCH_FAILED: ErrorCode = ErrorCode::new(3102, "app.launchFailed", 9);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[NOT_INSTALLED, INSTALL_FAILED, LAUNCH_FAILED];
