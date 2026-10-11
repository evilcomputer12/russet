// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `flutter` error codes (4120–4149). Owner: W17, W24.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 7 (build failed), except `flutter.notFound` (3).

use crate::error::ErrorCode;

/// 4120 `flutter.notFound`: No Flutter SDK was found.
pub const NOT_FOUND: ErrorCode = ErrorCode::new(4120, "flutter.notFound", 3);
/// 4121 `flutter.engineMissing`: The Flutter engine artifacts are missing.
pub const ENGINE_MISSING: ErrorCode = ErrorCode::new(4121, "flutter.engineMissing", 7);
/// 4122 `flutter.genSnapshotMissing`: No `gen_snapshot` for this engine.
pub const GEN_SNAPSHOT_MISSING: ErrorCode = ErrorCode::new(4122, "flutter.genSnapshotMissing", 7);
/// 4123 `flutter.customDevicesDisabled`: Flutter custom devices are disabled.
pub const CUSTOM_DEVICES_DISABLED: ErrorCode = ErrorCode::new(4123, "flutter.customDevicesDisabled", 7);
/// 4124 `flutter.pluginUnsupported`: A Flutter plugin is not supported.
pub const PLUGIN_UNSUPPORTED: ErrorCode = ErrorCode::new(4124, "flutter.pluginUnsupported", 7);
/// 4125 `flutter.cocoapodsRequired`: The project needs CocoaPods.
pub const COCOAPODS_REQUIRED: ErrorCode = ErrorCode::new(4125, "flutter.cocoapodsRequired", 7);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] =
    &[NOT_FOUND, ENGINE_MISSING, GEN_SNAPSHOT_MISSING, CUSTOM_DEVICES_DISABLED, PLUGIN_UNSUPPORTED, COCOAPODS_REQUIRED];
