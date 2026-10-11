// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `build` error codes (4000–4099). Owner: W5.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 7 (build failed).

use crate::error::ErrorCode;

/// 4000 `build.projectAmbiguous`: Several stacks match the project directory. Was E_PROJECT_AMBIGUOUS.
pub const PROJECT_AMBIGUOUS: ErrorCode = ErrorCode::new(4000, "build.projectAmbiguous", 7);
/// 4001 `build.inProgress`: A build of this project is already running. Was E_BUILD_IN_PROGRESS.
pub const IN_PROGRESS: ErrorCode = ErrorCode::new(4001, "build.inProgress", 7);
/// 4002 `build.compileFailed`: Compilation failed.
pub const COMPILE_FAILED: ErrorCode = ErrorCode::new(4002, "build.compileFailed", 7);
/// 4003 `build.linkFailed`: Linking failed.
pub const LINK_FAILED: ErrorCode = ErrorCode::new(4003, "build.linkFailed", 7);
/// 4004 `build.dynamicProductUnsupported`: A dynamic library product is not supported yet.
pub const DYNAMIC_PRODUCT_UNSUPPORTED: ErrorCode = ErrorCode::new(4004, "build.dynamicProductUnsupported", 7);
/// 4005 `build.bundleIdInvalid`: The bundle identifier is invalid.
pub const BUNDLE_ID_INVALID: ErrorCode = ErrorCode::new(4005, "build.bundleIdInvalid", 7);
/// 4006 `build.lintFailed`: A build lint failed.
pub const LINT_FAILED: ErrorCode = ErrorCode::new(4006, "build.lintFailed", 7);
/// 4007 `build.packageFailed`: IPA packaging failed.
pub const PACKAGE_FAILED: ErrorCode = ErrorCode::new(4007, "build.packageFailed", 7);
/// 4008 `build.resourceUnsupported`: A resource type is not supported.
pub const RESOURCE_UNSUPPORTED: ErrorCode = ErrorCode::new(4008, "build.resourceUnsupported", 7);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    PROJECT_AMBIGUOUS,
    IN_PROGRESS,
    COMPILE_FAILED,
    LINK_FAILED,
    DYNAMIC_PRODUCT_UNSUPPORTED,
    BUNDLE_ID_INVALID,
    LINT_FAILED,
    PACKAGE_FAILED,
    RESOURCE_UNSUPPORTED,
];
