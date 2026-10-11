// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `toolchain` handlers (§8.1). Owner: W3.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::toolchain as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ToolchainList, _, _>(not_implemented::<p::ToolchainList>);
    r.request::<p::ToolchainResolve, _, _>(not_implemented::<p::ToolchainResolve>);
    r.request::<p::ToolchainInstall, _, _>(not_implemented::<p::ToolchainInstall>);
    r.request::<p::ToolchainUninstall, _, _>(not_implemented::<p::ToolchainUninstall>);
    r.request::<p::ToolchainGc, _, _>(not_implemented::<p::ToolchainGc>);
    r.request::<p::ToolchainPinsUpdate, _, _>(not_implemented::<p::ToolchainPinsUpdate>);
}
