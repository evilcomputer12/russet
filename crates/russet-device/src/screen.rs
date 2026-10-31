// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Screenshots, the live mirror and HID input (W10; device.md §6.8).
//!
//! Screenshots are PNGs of the whole screen from CoreDevice's screen capture service, or from the DVT screenshot
//! service behind the developer hub when that cannot be opened (device.md §6.8.1; about 6 and 7.5 frames/s). A
//! poll mirror asks for frame after frame, so each device keeps its service open in a task of its own, which
//! closes after [`IDLE`] without a request or at the first error (the next request opens a fresh one). Every
//! step has a [`STEP`] limit, so a silent device fails a request with the step's name instead of hanging it. The
//! HEVC display stream and input are still to come.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use idevice::core_device::{ImageFormat as CaptureFormat, ScreenCaptureServiceClient};
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::dvt::screenshot::ScreenshotClient;
use idevice::{IdeviceError, RsdService as _};
use russet_core::Result;
use russet_core::error::codes;
use tokio::sync::{mpsc, oneshot};

use crate::coredevice::service;
use crate::error::{device_error, from_idevice};
use crate::io::ServiceConnector;
use crate::manager::DeviceHandle;
use crate::model::Udid;

/// A screenshot (PNG) and its size in pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Mirror options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirrorOptions {}

/// A running mirror session.
#[derive(Debug)]
pub struct MirrorHandle {
    _f0: (),
}

/// One HID input event.
#[derive(Debug, Clone, PartialEq)]
pub struct InputEvent {
    _f0: (),
}

/// How long a device's screenshot connection stays open without a request.
const IDLE: Duration = Duration::from_secs(15);

/// How long one step of a screenshot service may take before the request fails with that step's name.
const STEP: Duration = Duration::from_secs(15);

type Reply = oneshot::Sender<Result<Vec<u8>>>;

/// The request queue of each device's screenshot task, by UDID.
fn sessions() -> &'static Mutex<HashMap<String, mpsc::Sender<Reply>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, mpsc::Sender<Reply>>>> = OnceLock::new();
    SESSIONS.get_or_init(Default::default)
}

/// A screenshot of the device's screen.
pub(crate) async fn screenshot(h: &DeviceHandle) -> Result<Screenshot> {
    let key = h.udid().as_str().to_owned();
    let queue = {
        let mut map = sessions().lock().map_err(|_| device_error(codes::core::INTERNAL, "screenshot sessions"))?;
        map.retain(|_, q| !q.is_closed());
        map.get(&key).cloned()
    };
    let queue = match queue {
        Some(q) => q,
        None => {
            let (tx, rx) = mpsc::channel(4);
            tokio::spawn(serve(h.conn().clone(), h.udid().clone(), rx));
            if let Ok(mut map) = sessions().lock() {
                map.insert(key, tx.clone());
            }
            tx
        }
    };
    let (reply, answer) = oneshot::channel();
    let gone = || device_error(codes::device::PROTOCOL, "the screenshot connection closed; try again");
    queue.send(reply).await.map_err(|_| gone())?;
    let png = answer.await.map_err(|_| gone())??;
    let (width, height) = png_size(&png)?;
    Ok(Screenshot { png, width, height })
}

/// One device's screenshot task: CoreDevice's screen capture service (device.md §6.8.1), or the DVT screenshot
/// service when that cannot be opened; then a frame per request until [`IDLE`] or an error.
async fn serve(conn: Arc<dyn ServiceConnector>, udid: Udid, mut requests: mpsc::Receiver<Reply>) {
    let core = match timed("opening the screen capture service", conn.rsd_service(&udid, service::SCREEN_CAPTURE)).await
    {
        Ok(stream) => {
            step("starting the screen capture service", ScreenCaptureServiceClient::from_stream(stream)).await
        }
        Err(e) => Err(e),
    };
    let unavailable = match core {
        Ok(mut client) => {
            tracing::info!("screenshots from CoreDevice's screen capture service");
            while let Ok(Some(reply)) = tokio::time::timeout(IDLE, requests.recv()).await {
                let frame = step("capturing the screen", client.take_screenshot(None, CaptureFormat::Png)).await;
                let failed = frame.is_err();
                let _ = reply.send(frame);
                if failed {
                    return;
                }
            }
            return;
        }
        Err(e) => e,
    };
    tracing::info!(reason = %unavailable.message(), "no CoreDevice screen capture; trying the DVT screenshot service");
    let mut hub =
        match timed("opening the developer service hub", conn.rsd_service(&udid, service::DT_SERVICE_HUB)).await {
            Ok(stream) => RemoteServerClient::new(stream),
            Err(e) => return refuse_all(&mut requests, &unavailable, &e).await,
        };
    // The hub greets first (device.md Appendix B), before any channel opens.
    let opened = match step("the developer service hub's greeting", hub.read_message(0)).await {
        Ok(_) => step("opening the DVT screenshot channel", ScreenshotClient::new(&mut hub)).await,
        Err(e) => Err(e),
    };
    let mut client = match opened {
        Ok(c) => c,
        Err(e) => return refuse_all(&mut requests, &unavailable, &e).await,
    };
    tracing::info!("screenshots from the DVT screenshot service");
    while let Ok(Some(reply)) = tokio::time::timeout(IDLE, requests.recv()).await {
        let frame = step("taking the DVT screenshot", client.take_screenshot()).await;
        let failed = frame.is_err();
        let _ = reply.send(frame);
        if failed {
            return;
        }
    }
}

/// Answers every queued request with why neither screenshot service opened.
async fn refuse_all(requests: &mut mpsc::Receiver<Reply>, core: &russet_core::Error, dvt: &russet_core::Error) {
    let message = format!("no screenshot service: {}; {}", core.message(), dvt.message());
    while let Ok(Some(reply)) = tokio::time::timeout(Duration::from_millis(100), requests.recv()).await {
        let error = russet_core::Error::new(dvt.code(), message.clone()).with_hint(
            "Check that the device is unlocked and its developer disk image is mounted (`russet device ddi status`).",
        );
        let _ = reply.send(Err(error));
    }
}

/// A device step that fails, naming itself, instead of hanging.
async fn step<T>(what: &str, f: impl Future<Output = std::result::Result<T, IdeviceError>>) -> Result<T> {
    timed(what, async { f.await.map_err(|e| from_idevice(e, what)) }).await
}

/// [`step`] for steps that already answer a Russet error.
async fn timed<T>(what: &str, f: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(STEP, f).await.unwrap_or_else(|_| {
        Err(device_error(codes::device::TIMEOUT, format!("{what}: no answer within {} s", STEP.as_secs())))
    })
}

/// The width and height of a PNG, from its IHDR chunk.
pub(crate) fn png_size(png: &[u8]) -> Result<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || &png[..8] != SIGNATURE || &png[12..16] != b"IHDR" {
        return Err(device_error(codes::device::PROTOCOL, "the device's screenshot is not a PNG"));
    }
    let be = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
    Ok((be(16), be(20)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_png_size_and_refuses_anything_else() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&1170u32.to_be_bytes());
        png.extend_from_slice(&2532u32.to_be_bytes());
        assert_eq!(png_size(&png).ok(), Some((1170, 2532)));
        assert!(png_size(&png[..20]).is_err());
        assert!(png_size(b"\xff\xd8\xff\xe0 a JPEG, not a PNG at all").is_err());
    }
}
