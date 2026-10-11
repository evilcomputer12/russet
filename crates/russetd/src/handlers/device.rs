// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `device` handlers (§8.2). Owner: W4 (Wave 1), W10 (Wave 2), W30 (Wi-Fi).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::device as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::DeviceList, _, _>(not_implemented::<p::DeviceList>);
    r.request::<p::DeviceWatch, _, _>(not_implemented::<p::DeviceWatch>);
    r.request::<p::DeviceInfo, _, _>(not_implemented::<p::DeviceInfo>);
    r.request::<p::DeviceReadiness, _, _>(not_implemented::<p::DeviceReadiness>);
    r.request::<p::DevicePrepare, _, _>(not_implemented::<p::DevicePrepare>);
    r.request::<p::DevicePair, _, _>(not_implemented::<p::DevicePair>);
    r.request::<p::DeviceUnpair, _, _>(not_implemented::<p::DeviceUnpair>);
    r.request::<p::DeviceDeveloperModeReveal, _, _>(not_implemented::<p::DeviceDeveloperModeReveal>);
    r.request::<p::DeviceDdiStatus, _, _>(not_implemented::<p::DeviceDdiStatus>);
    r.request::<p::DeviceDdiList, _, _>(not_implemented::<p::DeviceDdiList>);
    r.request::<p::DeviceDdiMount, _, _>(not_implemented::<p::DeviceDdiMount>);
    r.request::<p::DeviceReserve, _, _>(not_implemented::<p::DeviceReserve>);
    r.request::<p::DeviceAliasSet, _, _>(not_implemented::<p::DeviceAliasSet>);
    r.request::<p::DeviceWifiEnable, _, _>(not_implemented::<p::DeviceWifiEnable>);
}
