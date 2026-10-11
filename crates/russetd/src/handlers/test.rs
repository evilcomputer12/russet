// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `test` handlers (§8.5). Owner: W32 (Wave 4), W28 (Wave 6).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::test as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::TestList, _, _>(not_implemented::<p::TestList>);
    r.request::<p::TestRun, _, _>(not_implemented::<p::TestRun>);
}
