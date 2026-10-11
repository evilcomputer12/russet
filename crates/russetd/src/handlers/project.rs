// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `project` handlers (§8.3). Owner: W5 (Wave 1), W16 (project.init).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::project as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ProjectDetect, _, _>(not_implemented::<p::ProjectDetect>);
    r.request::<p::ProjectInfo, _, _>(not_implemented::<p::ProjectInfo>);
    r.request::<p::ProjectInit, _, _>(not_implemented::<p::ProjectInit>);
    r.request::<p::ProjectInfoPlist, _, _>(not_implemented::<p::ProjectInfoPlist>);
}
