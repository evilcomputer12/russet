// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Jobs: queue, locks (`LockKey`), limits, the cancellation tree, persistence, retention (ARCHITECTURE §6.2).
//!
//! F0 stub (W1); W2 owns it.

use std::sync::Arc;

/// A cheap-to-clone handle (`DaemonCtx.jobs`).
#[derive(Debug, Clone, Default)]
pub struct JobManager {
    _f0: Arc<()>,
}
