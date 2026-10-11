// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `job` handlers (§8.1). Owner: W2.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::job as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::JobList, _, _>(not_implemented::<p::JobList>);
    r.request::<p::JobGet, _, _>(not_implemented::<p::JobGet>);
    r.request::<p::JobWait, _, _>(not_implemented::<p::JobWait>);
    r.request::<p::JobCancel, _, _>(not_implemented::<p::JobCancel>);
    r.request::<p::JobSubscribe, _, _>(not_implemented::<p::JobSubscribe>);
    r.request::<p::JobLog, _, _>(not_implemented::<p::JobLog>);
}
