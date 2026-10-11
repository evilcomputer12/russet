// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `daemon` handlers (§8.1). Owner: W2.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::daemon as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::DaemonPing, _, _>(not_implemented::<p::DaemonPing>);
    r.request::<p::DaemonStatus, _, _>(not_implemented::<p::DaemonStatus>);
    r.request::<p::DaemonShutdown, _, _>(not_implemented::<p::DaemonShutdown>);
    r.request::<p::DaemonSetLogFilter, _, _>(not_implemented::<p::DaemonSetLogFilter>);
    r.request::<p::DaemonLogsSubscribe, _, _>(not_implemented::<p::DaemonLogsSubscribe>);
}
