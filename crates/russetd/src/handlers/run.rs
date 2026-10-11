// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `run` handlers (§8.5). Owner: W7 (Wave 1), W11 (Wave 2), W18 (Flutter, Wave 3).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::run as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::RunStart, _, _>(not_implemented::<p::RunStart>);
    r.request::<p::RunList, _, _>(not_implemented::<p::RunList>);
    r.request::<p::RunSubscribe, _, _>(not_implemented::<p::RunSubscribe>);
    r.request::<p::RunOutputQuery, _, _>(not_implemented::<p::RunOutputQuery>);
    r.request::<p::RunStop, _, _>(not_implemented::<p::RunStop>);
    r.request::<p::RunRelaunch, _, _>(not_implemented::<p::RunRelaunch>);
    r.request::<p::RunHotReload, _, _>(not_implemented::<p::RunHotReload>);
}
