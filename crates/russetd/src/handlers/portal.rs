// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `portal` handlers (§8.4). Owner: W6 (Wave 1), W12 (Wave 2).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::portal as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::PortalCertificatesList, _, _>(not_implemented::<p::PortalCertificatesList>);
    r.request::<p::PortalDevicesList, _, _>(not_implemented::<p::PortalDevicesList>);
    r.request::<p::PortalBundleIdsList, _, _>(not_implemented::<p::PortalBundleIdsList>);
    r.request::<p::PortalProfilesList, _, _>(not_implemented::<p::PortalProfilesList>);
    r.request::<p::PortalDevicesRegister, _, _>(not_implemented::<p::PortalDevicesRegister>);
    r.request::<p::PortalCertificatesRevoke, _, _>(not_implemented::<p::PortalCertificatesRevoke>);
}
