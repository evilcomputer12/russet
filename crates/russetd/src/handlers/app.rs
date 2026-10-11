// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `app` handlers (§8.2). Owner: W4 (Wave 1), W10 (app.icon).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::app as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::AppList, _, _>(not_implemented::<p::AppList>);
    r.request::<p::AppInstall, _, _>(not_implemented::<p::AppInstall>);
    r.request::<p::AppUninstall, _, _>(not_implemented::<p::AppUninstall>);
    r.request::<p::AppLaunch, _, _>(not_implemented::<p::AppLaunch>);
    r.request::<p::AppTerminate, _, _>(not_implemented::<p::AppTerminate>);
    r.request::<p::AppIcon, _, _>(not_implemented::<p::AppIcon>);
}
