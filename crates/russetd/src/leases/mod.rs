// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Leases and sessions with the 2 s grace (ARCHITECTURE §6.3).
//!
//! F0 stub (W1); W2 owns it.

use std::sync::Arc;

/// A cheap-to-clone handle (`DaemonCtx.leases`).
#[derive(Debug, Clone, Default)]
pub struct LeaseTable {
    _f0: Arc<()>,
}
