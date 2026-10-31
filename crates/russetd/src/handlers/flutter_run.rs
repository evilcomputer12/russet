// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `flutter_run` handlers (§8.5). Owner: W18.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::flutter_run as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::FlutterRun, _, _>(not_implemented::<p::FlutterRun>);
    r.request::<p::FlutterAttach, _, _>(not_implemented::<p::FlutterAttach>);
    r.request::<p::FlutterDeviceRunDebug, _, _>(not_implemented::<p::FlutterDeviceRunDebug>);
    r.request::<p::FlutterDeviceForward, _, _>(not_implemented::<p::FlutterDeviceForward>);
}
