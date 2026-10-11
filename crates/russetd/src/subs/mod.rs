// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Subscriptions with bounded queues and drop counters (ARCHITECTURE §6.4).
//!
//! F0 stub (W1); W2 owns it.

use std::sync::Arc;

/// A cheap-to-clone handle (`DaemonCtx.subs`).
#[derive(Debug, Clone, Default)]
pub struct SubscriptionHub {
    _f0: Arc<()>,
}
