// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `approval` handlers (§8.1). Owner: W2.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::approval as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ApprovalList, _, _>(not_implemented::<p::ApprovalList>);
    r.request::<p::ApprovalGet, _, _>(not_implemented::<p::ApprovalGet>);
    r.request::<p::ApprovalGrant, _, _>(not_implemented::<p::ApprovalGrant>);
    r.request::<p::ApprovalDeny, _, _>(not_implemented::<p::ApprovalDeny>);
}
