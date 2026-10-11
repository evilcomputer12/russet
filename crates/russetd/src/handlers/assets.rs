// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `assets` handlers (§8.3). Owner: W21.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::assets as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::AssetsList, _, _>(not_implemented::<p::AssetsList>);
    r.request::<p::AssetsCompile, _, _>(not_implemented::<p::AssetsCompile>);
    r.request::<p::AssetsAppIcon, _, _>(not_implemented::<p::AssetsAppIcon>);
    r.request::<p::AssetsRender, _, _>(not_implemented::<p::AssetsRender>);
}
