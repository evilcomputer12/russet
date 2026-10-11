// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `provision` error codes (5200–5249). Owner: W6, W12.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 8 (signing or provisioning failed).

use crate::error::ErrorCode;

/// 5200 `provision.certLimit`: The team reached its certificate limit.
pub const CERT_LIMIT: ErrorCode = ErrorCode::new(5200, "provision.certLimit", 8);
/// 5201 `provision.devicePending`: The device registration is pending.
pub const DEVICE_PENDING: ErrorCode = ErrorCode::new(5201, "provision.devicePending", 8);
/// 5202 `provision.deviceLimit`: The team reached its device limit.
pub const DEVICE_LIMIT: ErrorCode = ErrorCode::new(5202, "provision.deviceLimit", 8);
/// 5203 `provision.portalStepRequired`: A capability needs a manual step in the developer portal.
pub const PORTAL_STEP_REQUIRED: ErrorCode = ErrorCode::new(5203, "provision.portalStepRequired", 8);
/// 5204 `provision.freeQuota`: The free team's quota is exhausted.
pub const FREE_QUOTA: ErrorCode = ErrorCode::new(5204, "provision.freeQuota", 8);
/// 5205 `provision.freeUnsupported`: Not available to free teams.
pub const FREE_UNSUPPORTED: ErrorCode = ErrorCode::new(5205, "provision.freeUnsupported", 8);
/// 5206 `provision.planStale`: The plan's digest no longer matches the portal state.
pub const PLAN_STALE: ErrorCode = ErrorCode::new(5206, "provision.planStale", 8);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] =
    &[CERT_LIMIT, DEVICE_PENDING, DEVICE_LIMIT, PORTAL_STEP_REQUIRED, FREE_QUOTA, FREE_UNSUPPORTED, PLAN_STALE];
