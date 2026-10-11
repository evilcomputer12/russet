// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `forward` handlers (§8.2). Owner: W4.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::forward as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ForwardOpen, _, _>(not_implemented::<p::ForwardOpen>);
    r.request::<p::ForwardClose, _, _>(not_implemented::<p::ForwardClose>);
    r.request::<p::ForwardList, _, _>(not_implemented::<p::ForwardList>);
}
