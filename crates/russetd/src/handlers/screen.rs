// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `screen` handlers (§8.2). Owner: W10.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::screen as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ScreenScreenshot, _, _>(not_implemented::<p::ScreenScreenshot>);
    r.request::<p::ScreenMirrorStart, _, _>(not_implemented::<p::ScreenMirrorStart>);
    r.request::<p::ScreenMirrorStop, _, _>(not_implemented::<p::ScreenMirrorStop>);
    r.request::<p::ScreenElements, _, _>(not_implemented::<p::ScreenElements>);
    r.request::<p::ScreenPress, _, _>(not_implemented::<p::ScreenPress>);
}
