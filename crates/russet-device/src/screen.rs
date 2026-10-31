// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Screenshots, the live mirror and HID input (W10; device.md §6.8).
//!
//! Screenshots come from the DVT screenshot service behind the developer hub: a PNG of the whole screen, about
//! 7.5 frames/s (device.md §2, row 9). A poll mirror asks for frame after frame, so each device keeps one hub
//! connection and one screenshot channel in a task of its own, which closes after [`IDLE`] without a request or
//! at the first error (the next request opens a fresh one). The HEVC display stream and input are still to come.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use idevice::ReadWrite;
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::dvt::screenshot::ScreenshotClient;
use russet_core::Result;
use russet_core::error::codes;
use tokio::sync::{mpsc, oneshot};

use crate::coredevice::service;
use crate::error::{device_error, from_idevice};
use crate::manager::DeviceHandle;

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
            let stream = h.conn().rsd_service(h.udid(), service::DT_SERVICE_HUB).await?;
            let (tx, rx) = mpsc::channel(4);
            tokio::spawn(serve(RemoteServerClient::new(stream), rx));
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

/// One device's screenshot task: the hub's hello, one screenshot channel, then a frame per request.
async fn serve(mut hub: RemoteServerClient<Box<dyn ReadWrite>>, mut requests: mpsc::Receiver<Reply>) {
    // The hub greets first (device.md Appendix B), before any channel opens.
    let opened = match hub.read_message(0).await {
        Ok(_) => ScreenshotClient::new(&mut hub).await.map_err(|e| from_idevice(e, "DVT screenshot service")),
        Err(e) => Err(from_idevice(e, "DVT hello")),
    };
    let mut client = match opened {
        Ok(c) => c,
        Err(e) => {
            // Every request already queued gets the reason (an error is not `Clone`: rebuild it).
            while let Ok(Some(reply)) = tokio::time::timeout(Duration::from_millis(100), requests.recv()).await {
                let mut copy = russet_core::Error::new(e.code(), e.message().to_owned());
                if let Some(hint) = e.hint() {
                    copy = copy.with_hint(hint.to_owned());
                }
                let _ = reply.send(Err(copy));
            }
            return;
        }
    };
    while let Ok(Some(reply)) = tokio::time::timeout(IDLE, requests.recv()).await {
        let frame = client.take_screenshot().await.map_err(|e| from_idevice(e, "DVT screenshot"));
        let failed = frame.is_err();
        let _ = reply.send(frame);
        if failed {
            return;
        }
    }
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
