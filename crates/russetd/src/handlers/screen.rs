// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `screen` handlers (§8.2). Owner: W10.
//!
//! `screen.screenshot` answers from the device's screen capture service (a poll mirror calls it frame after frame;
//! the device keeps the connection open between them). `screen.mirror.*` streams the device's display session to
//! one WebSocket viewer ([`crate::web::mirror`]). UI elements and button presses are still F0 stubs answering
//! `core.notImplemented` (1005).

use russet_core::Result;
use russet_device::rpc;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::screen as p;

use super::device::{caller, manager};
use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ScreenScreenshot, _, _>(screen_screenshot);
    r.request::<p::ScreenMirrorStart, _, _>(screen_mirror_start);
    r.request::<p::ScreenMirrorStop, _, _>(screen_mirror_stop);
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

async fn screen_mirror_start(
    cx: DaemonCtx,
    rq: RequestCtx,
    p: p::ScreenMirrorStartParams,
) -> Result<p::ScreenMirrorStartResult> {
    if !p.codecs.is_empty() && !p.codecs.contains(&p::MirrorCodec::Hevc) {
        return Err(russet_core::Error::new(
            russet_core::error::codes::screen::CODEC_UNSUPPORTED,
            "the daemon streams HEVC only; a viewer without it polls screen.screenshot",
        ));
    }
    let m = manager(&cx).await?;
    let input = p.input == Some(true);
    let device = match input {
        true => rpc::handle_for(m.as_ref(), p.udid.as_deref(), &caller(&rq))?,
        false => rpc::handle(m.as_ref(), p.udid.as_deref())?,
    };
    crate::web::mirror::start(&cx.web, &device, input).await
}

async fn screen_mirror_stop(
    _cx: DaemonCtx,
    _rq: RequestCtx,
    p: p::ScreenMirrorStopParams,
) -> Result<p::ScreenMirrorStopResult> {
    crate::web::mirror::stop(&p.stream_id);
    Ok(p::ScreenMirrorStopResult {})
}
