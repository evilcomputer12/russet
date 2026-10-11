// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Out-of-band approvals (ARCHITECTURE §6.5, §11.3).
//!
//! F0 stub (W1); W2 owns it.

use std::sync::Arc;

/// A cheap-to-clone handle (`DaemonCtx.approvals`).
#[derive(Debug, Clone, Default)]
pub struct ApprovalService {
    _f0: Arc<()>,
}
