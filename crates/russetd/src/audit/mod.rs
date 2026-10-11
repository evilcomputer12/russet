// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The append-only `state/audit.jsonl` (ARCHITECTURE §6.5).
//!
//! F0 stub (W1); W2 owns it.

use std::sync::Arc;

/// A cheap-to-clone handle (`DaemonCtx.audit`).
#[derive(Debug, Clone, Default)]
pub struct AuditLog {
    _f0: Arc<()>,
}
