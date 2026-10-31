// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `flutter_build` handlers (§8.3). Owner: W17 (Wave 3), W24 (genSnapshot rows).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::flutter_build as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::FlutterRegister, _, _>(not_implemented::<p::FlutterRegister>);
    r.request::<p::FlutterDeviceInstall, _, _>(not_implemented::<p::FlutterDeviceInstall>);
    r.request::<p::FlutterGenSnapshotEnsure, _, _>(not_implemented::<p::FlutterGenSnapshotEnsure>);
}
