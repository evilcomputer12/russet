// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `config` handlers (§8.1). Owner: W3.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::config as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ConfigGet, _, _>(not_implemented::<p::ConfigGet>);
    r.request::<p::ConfigSet, _, _>(not_implemented::<p::ConfigSet>);
    r.request::<p::ConfigUnset, _, _>(not_implemented::<p::ConfigUnset>);
    r.request::<p::ConfigSchema, _, _>(not_implemented::<p::ConfigSchema>);
}
