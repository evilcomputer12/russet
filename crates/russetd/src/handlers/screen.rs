// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `screen` handlers (§8.2). Owner: W10.
//!
//! `screen.screenshot` answers from the device's DVT screenshot service (a poll mirror calls it frame after
//! frame; the device keeps the connection open between them). The display stream (`screen.mirror.*`), UI
//! elements and button presses are still F0 stubs answering `core.notImplemented` (1005).

use russet_core::Result;
use russet_device::rpc;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::screen as p;

use super::device::manager;
use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ScreenScreenshot, _, _>(screen_screenshot);
    r.request::<p::ScreenMirrorStart, _, _>(not_implemented::<p::ScreenMirrorStart>);
    r.request::<p::ScreenMirrorStop, _, _>(not_implemented::<p::ScreenMirrorStop>);
    r.request::<p::ScreenElements, _, _>(not_implemented::<p::ScreenElements>);
    r.request::<p::ScreenPress, _, _>(not_implemented::<p::ScreenPress>);
}

async fn screen_screenshot(
    cx: DaemonCtx,
    _rq: RequestCtx,
    p: p::ScreenScreenshotParams,
) -> Result<p::ScreenScreenshotResult> {
    rpc::screen_screenshot(manager(&cx).await?.as_ref(), p).await
}
