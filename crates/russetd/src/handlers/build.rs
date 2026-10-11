// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `build` handlers (§8.3). Owner: W5 (Wave 1), W16 (build.settings).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::build as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::BuildStart, _, _>(not_implemented::<p::BuildStart>);
    r.request::<p::BuildPlan, _, _>(not_implemented::<p::BuildPlan>);
    r.request::<p::BuildSettings, _, _>(not_implemented::<p::BuildSettings>);
    r.request::<p::BuildClean, _, _>(not_implemented::<p::BuildClean>);
    r.request::<p::BuildInspect, _, _>(not_implemented::<p::BuildInspect>);
}
