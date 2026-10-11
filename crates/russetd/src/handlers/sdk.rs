// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `sdk` handlers (§8.1). Owner: W3.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::sdk as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::SdkList, _, _>(not_implemented::<p::SdkList>);
    r.request::<p::SdkImport, _, _>(not_implemented::<p::SdkImport>);
    r.request::<p::SdkRemove, _, _>(not_implemented::<p::SdkRemove>);
    r.request::<p::SdkDeviceSupportCheck, _, _>(not_implemented::<p::SdkDeviceSupportCheck>);
    r.request::<p::SdkDeviceSupportFetch, _, _>(not_implemented::<p::SdkDeviceSupportFetch>);
}
