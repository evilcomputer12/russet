// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `ib` handlers (§8.3). Owner: W21b (Wave 4), W28b and W28c (Wave 6).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::ib as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::IbCompile, _, _>(not_implemented::<p::IbCompile>);
    r.request::<p::IbDump, _, _>(not_implemented::<p::IbDump>);
}
