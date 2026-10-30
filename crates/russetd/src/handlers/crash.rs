// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `crash` handlers (§8.5). Owner: W11.
//!
//! `crash.list`/`crash.symbolicate` front the local crash store (`russet_debug::crash::CrashStore`). `crash.fetch`
//! pulls new reports off the device (DEV-15, W10) and ingests them; the device pull is device-gated (DC-10).

use std::path::PathBuf;

use russet_core::Result;
use russet_debug::crash::CrashStore;
use russet_device::{DeviceSelector, Udid};
use russet_ipc::{RequestCtx, Router};
use russet_protocol::crash as p;

use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::CrashList, _, _>(list);
    r.request::<p::CrashFetch, _, _>(fetch);
    r.request::<p::CrashSymbolicate, _, _>(symbolicate);
}

async fn list(cx: DaemonCtx, _rq: RequestCtx, params: p::CrashListParams) -> Result<p::CrashListResult> {
    let store = CrashStore::new(&cx.dirs.data);
    let crashes = store.list(params.bundle_id.as_deref(), params.since.as_deref(), params.limit.map(|l| l as usize));
    Ok(p::CrashListResult { crashes })
}

async fn fetch(cx: DaemonCtx, _rq: RequestCtx, params: p::CrashFetchParams) -> Result<p::CrashFetchResult> {
    let sel = match &params.udid {
        Some(u) => DeviceSelector::Udid(Udid(u.clone())),
        None => DeviceSelector::Only,
    };
    let dev = cx.debug.get()?.devices();
    let udid = dev.resolve(&sel).await?;
    let reports = dev.fetch_crash_reports(&udid, params.bundle_id.as_deref(), params.since.as_deref()).await?;
    let store = CrashStore::new(&cx.dirs.data);
    let mut fetched = Vec::new();
    for report in &reports {
        fetched.push(store.ingest(report)?);
    }
    Ok(p::CrashFetchResult { fetched })
}

async fn symbolicate(
    cx: DaemonCtx,
    _rq: RequestCtx,
    params: p::CrashSymbolicateParams,
) -> Result<p::CrashSymbolicateResult> {
    let store = CrashStore::new(&cx.dirs.data);
    let path = params.path.as_ref().map(PathBuf::from);
    let crash = store.symbolicate(params.crash_id.as_deref(), path.as_deref())?;
    Ok(match params.format.unwrap_or_default() {
        p::CrashFormat::Json => p::CrashSymbolicateResult::Json { crash },
        p::CrashFormat::Text => p::CrashSymbolicateResult::Text { crash: russet_debug::crash::render_text(&crash) },
    })
}
