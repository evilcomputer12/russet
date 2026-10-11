// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `sdk` handlers (ARCHITECTURE §8.1, §16.3). Owner: W3 (`sdk.list`); W16b (`sdk.import`, `sdk.remove`); W33
//! (`sdk.deviceSupport.*`, deferred). The import, remove and Device Support methods answer
//! `core.notImplemented` until their cards land.

use russet_core::error::Result;
use russet_ipc::Router;
use russet_protocol::sdk as p;
use russet_toolchain::Toolchains;

use super::config::toolchains_of;
use super::not_implemented;
use super::toolchain::wire_sdk;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::SdkList, _, _>(|cx: DaemonCtx, _rq, _params| async move { list(&*toolchains_of(&cx)?).await });
    r.request::<p::SdkImport, _, _>(not_implemented::<p::SdkImport>);
    r.request::<p::SdkRemove, _, _>(not_implemented::<p::SdkRemove>);
    r.request::<p::SdkDeviceSupportCheck, _, _>(not_implemented::<p::SdkDeviceSupportCheck>);
    r.request::<p::SdkDeviceSupportFetch, _, _>(not_implemented::<p::SdkDeviceSupportFetch>);
}

/// `sdk.list`: Russet's imports first, then xtool bundles read in place.
pub async fn list(toolchains: &Toolchains) -> Result<p::SdkListResult> {
    Ok(p::SdkListResult { sdks: toolchains.sdks().await?.iter().map(wire_sdk).collect() })
}
