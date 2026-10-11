// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Shared wire types (`LogEntry`, `DeviceSummary`, `JitStats`, `SessionSummary`, …; ARCHITECTURE §9.5). No methods. Owner: W2.
//!
//! F0 stub (W1): no methods; W2 adds the shared DTOs of ARCHITECTURE §9.5 here.

use crate::{MethodInfo, NotificationInfo};

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
