// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `screen` error codes (8000–8099). Owner: W10.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! ARCHITECTURE §13.4 lists no exit code for this family; W1 uses 6 (device state), like the device plane it belongs to.

use crate::error::ErrorCode;

/// 8000 `screen.mediaInUse`: Another client holds the display stream. Was E_MEDIA_IN_USE (device 9022).
pub const MEDIA_IN_USE: ErrorCode = ErrorCode::new(8000, "screen.mediaInUse", 6);
/// 8001 `screen.streamFailed`: The screen stream failed.
pub const STREAM_FAILED: ErrorCode = ErrorCode::new(8001, "screen.streamFailed", 6);
/// 8002 `screen.codecUnsupported`: No supported codec.
pub const CODEC_UNSUPPORTED: ErrorCode = ErrorCode::new(8002, "screen.codecUnsupported", 6);
/// 8003 `screen.inputDenied`: Input injection was denied.
pub const INPUT_DENIED: ErrorCode = ErrorCode::new(8003, "screen.inputDenied", 6);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[MEDIA_IN_USE, STREAM_FAILED, CODEC_UNSUPPORTED, INPUT_DENIED];
