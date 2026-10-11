// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `sign` handlers (§8.4). Owner: W6.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::sign as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::SignRun, _, _>(not_implemented::<p::SignRun>);
    r.request::<p::SignVerify, _, _>(not_implemented::<p::SignVerify>);
    r.request::<p::SignLint, _, _>(not_implemented::<p::SignLint>);
    r.request::<p::SignInspect, _, _>(not_implemented::<p::SignInspect>);
}
