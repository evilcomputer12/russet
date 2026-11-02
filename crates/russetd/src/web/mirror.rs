// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `/mirror/v1`: the `russet.mirror.v1` WebSocket (ARCHITECTURE §10.1) [D 10]. Owner: W10.
//!
//! `screen.mirror.start` opens the device's display session, waits for its first keyframe (whose SPS gives the
//! codec string and the screen size), and parks the session here under a `str_…` id with a single-use token. The
//! viewer connects within [`CONNECT_WITHIN`], sends `{"t":"auth","token":…}` within [`AUTH_WITHIN`], and receives
//! a `config` message, then one binary message per access unit. Access units are Annex-B (`format: "annexb"`):
//! WebCodecs decodes them without an `hvcC` description. One viewer per stream; the display session ends when the
//! viewer leaves, on `screen.mirror.stop`, or when nobody claims it in time.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::extract::Query;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use russet_core::error::{Error, Result, codes};
use russet_core::ids::StreamId;
use russet_device::{
    AccessUnit, ButtonAction, DeviceHandle, HardwareButton, HevcConfig, InputEvent, MirrorHandle, MirrorOptions,
    TouchPhase, hevc_config,
};
use russet_protocol::screen as p;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::WebEndpoint;

const ROUTE: &str = "/mirror/v1";
const SUBPROTOCOL: &str = "russet.mirror.v1";
/// How long `screen.mirror.start` waits for the device's first keyframe.
const FIRST_KEYFRAME: Duration = Duration::from_secs(10);
/// How long a started stream waits for its viewer (and its token stays valid).
const CONNECT_WITHIN: Duration = Duration::from_secs(30);
/// How long a connected viewer has to authenticate.
const AUTH_WITHIN: Duration = Duration::from_secs(2);
/// Close codes: authentication failed or came late; the stream is unknown or already has its viewer.
const CLOSE_UNAUTHORIZED: u16 = 4401;
const CLOSE_UNKNOWN_STREAM: u16 = 4404;

/// A started stream: waiting for its viewer, or streaming to it (cancel to stop).
enum Slot {
    Waiting(Box<Parked>),
    Live(CancellationToken),
}

struct Parked {
    handle: MirrorHandle,
    first: AccessUnit,
    config: HevcConfig,
}

fn streams() -> &'static Mutex<HashMap<String, Slot>> {
    static STREAMS: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    STREAMS.get_or_init(Default::default)
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, Slot>> {
    streams().lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Registers the route; called while the daemon starts, before the endpoint does.
pub fn register(web: &WebEndpoint) -> Result<()> {
    let endpoint = web.clone();
    web.route(
        ROUTE,
        axum::routing::get(move |ws: WebSocketUpgrade, Query(q): Query<HashMap<String, String>>| {
            let endpoint = endpoint.clone();
            async move { upgrade(endpoint, ws, q) }
        }),
    )
}

/// `screen.mirror.start` for `device`: an HEVC stream, with input when `input`.
pub async fn start(web: &WebEndpoint, device: &DeviceHandle, input: bool) -> Result<p::ScreenMirrorStartResult> {
    let mut handle = device.mirror(MirrorOptions { input }).await?;
    let first = tokio::time::timeout(FIRST_KEYFRAME, async {
        while let Some(unit) = handle.frames.recv().await {
            if unit.key {
                return Some(unit);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| {
        Error::new(
            codes::screen::STREAM_FAILED,
            format!("the device sent no keyframe within {} s", FIRST_KEYFRAME.as_secs()),
        )
        .with_hint("Unlock the device and try again; the viewer falls back to screenshots meanwhile.")
    })?;
    let config = hevc_config(&first.data).ok_or_else(|| {
        Error::new(codes::screen::CODEC_UNSUPPORTED, "the device's first keyframe has no readable HEVC SPS")
    })?;
    let addr = web.ensure_started().await?;
    let id = StreamId::generate().to_string();
    let token = web.tokens().issue(ROUTE, id.clone(), CONNECT_WITHIN);
    let (width, height) = (config.width, config.height);
    lock().insert(id.clone(), Slot::Waiting(Box::new(Parked { handle, first, config })));
    let unclaimed = id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(CONNECT_WITHIN).await;
        let mut map = lock();
        if matches!(map.get(&unclaimed), Some(Slot::Waiting(_))) {
            map.remove(&unclaimed);
            tracing::info!("a mirror stream ended: no viewer connected");
        }
    });
    Ok(p::ScreenMirrorStartResult {
        url: format!("ws://{addr}{ROUTE}?stream={id}"),
        stream_id: id,
        token,
        codec: p::MirrorCodec::Hevc,
        width,
        height,
    })
}

/// `screen.mirror.stop`: ends the stream; a stream that already ended is no error.
pub fn stop(stream: &str) {
    match lock().remove(stream) {
        Some(Slot::Live(cancel)) => cancel.cancel(),
        Some(Slot::Waiting(_)) | None => {}
    }
}

fn upgrade(web: WebEndpoint, ws: WebSocketUpgrade, q: HashMap<String, String>) -> Response {
    let stream = q.get("stream").cloned().unwrap_or_default();
    ws.protocols([SUBPROTOCOL]).on_upgrade(move |socket| serve(web, socket, stream))
}

/// What the viewer sends (ARCHITECTURE §10.1).
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
enum ClientMessage {
    Auth {
        token: String,
    },
    Ping,
    Keyframe,
    Pause,
    Resume,
    Touch {
        phase: Phase,
        x: u16,
        y: u16,
    },
    Button {
        button: Button,
        #[serde(default)]
        action: Action,
    },
    Key {
        usage: u16,
        down: bool,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Phase {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Button {
    Home,
    Lock,
    VolumeUp,
    VolumeDown,
    Mute,
    Siri,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Action {
    #[default]
    Press,
    Down,
    Up,
}

impl ClientMessage {
    /// The device input this message asks for, if it is input.
    fn input(self) -> Option<InputEvent> {
        Some(match self {
            Self::Touch { phase, x, y } => {
                let phase = match phase {
                    Phase::Down => TouchPhase::Down,
                    Phase::Move => TouchPhase::Move,
                    Phase::Up => TouchPhase::Up,
                };
                InputEvent::Touch { phase, x, y }
            }
            Self::Button { button, action } => InputEvent::Button {
                button: match button {
                    Button::Home => HardwareButton::Home,
                    Button::Lock => HardwareButton::Lock,
                    Button::VolumeUp => HardwareButton::VolumeUp,
                    Button::VolumeDown => HardwareButton::VolumeDown,
                    Button::Mute => HardwareButton::Mute,
                    Button::Siri => HardwareButton::Siri,
                },
                action: match action {
                    Action::Press => ButtonAction::Press,
                    Action::Down => ButtonAction::Down,
                    Action::Up => ButtonAction::Up,
                },
            },
            Self::Key { usage, down } => InputEvent::Key { usage, down },
            Self::Text { text } => InputEvent::Text(text),
            _ => return None,
        })
    }
}

async fn close(mut socket: WebSocket, code: u16, reason: &str) {
    let _ = socket.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
}

async fn serve(web: WebEndpoint, mut socket: WebSocket, stream: String) {
    let token = match tokio::time::timeout(AUTH_WITHIN, socket.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str(t.as_str()) {
            Ok(ClientMessage::Auth { token }) => Some(token),
            _ => None,
        },
        _ => None,
    };
    if token.and_then(|t| web.tokens().redeem(&t, ROUTE)).as_deref() != Some(stream.as_str()) {
        return close(socket, CLOSE_UNAUTHORIZED, "authenticate with the stream's token first").await;
    }
    let cancel = CancellationToken::new();
    let parked = {
        let mut map = lock();
        match map.remove(&stream) {
            Some(Slot::Waiting(parked)) => {
                map.insert(stream.clone(), Slot::Live(cancel.clone()));
                Some(parked)
            }
            Some(live) => {
                map.insert(stream.clone(), live);
                None
            }
            None => None,
        }
    };
    let Some(parked) = parked else {
        return close(socket, CLOSE_UNKNOWN_STREAM, "this stream ended or already has a viewer").await;
    };
    tracing::info!("a mirror viewer connected");
    let ended = run(&mut socket, *parked, &cancel).await;
    if let Some((code, message)) = ended {
        let end = serde_json::json!({"t": "end", "code": code, "message": message});
        let _ = socket.send(Message::Text(end.to_string().into())).await;
    }
    let _ = socket.send(Message::Close(None)).await;
    lock().remove(&stream);
    tracing::info!("a mirror viewer left");
}

/// Streams until the viewer leaves or the stream is stopped (`None`), or the device's stream ends (its `end`).
async fn run(socket: &mut WebSocket, parked: Parked, cancel: &CancellationToken) -> Option<(&'static str, String)> {
    let Parked { mut handle, first, config } = parked;
    let input = handle.input_sender();
    let hello = serde_json::json!({
        "t": "config",
        "codec": config.codec,
        "codedWidth": config.coded_width,
        "codedHeight": config.coded_height,
        "width": config.width,
        "height": config.height,
        "rotation": 0,
        "fpsHint": 60,
        "format": "annexb",
        "input": input.is_some(),
    });
    socket.send(Message::Text(hello.to_string().into())).await.ok()?;
    socket.send(Message::Binary(frame(0, &first).into())).await.ok()?;
    let mut seq = 1u32;
    let mut paused = false;
    let mut need_key = false;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return None,
            unit = handle.frames.recv() => {
                let Some(unit) = unit else {
                    return Some(("E_STREAM_FAILED", "the device's display stream ended".to_owned()));
                };
                if paused || (need_key && !unit.key) {
                    continue;
                }
                need_key = false;
                socket.send(Message::Binary(frame(seq, &unit).into())).await.ok()?;
                seq = seq.wrapping_add(1);
            }
            message = socket.recv() => {
                let text = match message {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return None,
                    Some(Ok(_)) => continue,
                };
                let Ok(message) = serde_json::from_str::<ClientMessage>(text.as_str()) else { continue };
                match message {
                    ClientMessage::Ping => {
                        socket.send(Message::Text(r#"{"t":"pong"}"#.into())).await.ok()?;
                    }
                    ClientMessage::Pause => paused = true,
                    ClientMessage::Resume => {
                        paused = false;
                        need_key = true;
                    }
                    ClientMessage::Auth { .. } | ClientMessage::Keyframe => {}
                    other => {
                        if let (Some(tx), Some(event)) = (input.as_ref(), other.input()) {
                            // A full queue drops the event rather than stalling the picture.
                            let _ = tx.try_send(event);
                        }
                    }
                }
            }
        }
    }
}

/// One binary message: the 16-byte header of ARCHITECTURE §10.1, then the access unit.
fn frame(seq: u32, unit: &AccessUnit) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + unit.data.len());
    out.extend_from_slice(&[1, 1, u8::from(unit.key), 0]);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&unit.pts_micros.to_le_bytes());
    out.extend_from_slice(&unit.data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_carry_the_header_of_the_contract() {
        let unit = AccessUnit { data: vec![0, 0, 0, 1, 0x26], key: true, pts_micros: 0x0102 };
        let f = frame(7, &unit);
        assert_eq!(&f[..4], &[1, 1, 1, 0]);
        assert_eq!(u32::from_le_bytes(f[4..8].try_into().expect("seq")), 7);
        assert_eq!(u64::from_le_bytes(f[8..16].try_into().expect("pts")), 0x0102);
        assert_eq!(&f[16..], &unit.data[..]);
    }

    #[test]
    fn viewer_messages_parse_into_input() {
        let parse = |s: &str| serde_json::from_str::<ClientMessage>(s).expect(s);
        assert_eq!(
            parse(r#"{"t":"touch","phase":"down","x":100,"y":65535}"#).input(),
            Some(InputEvent::Touch { phase: TouchPhase::Down, x: 100, y: 65535 })
        );
        assert_eq!(
            parse(r#"{"t":"button","button":"volume-up"}"#).input(),
            Some(InputEvent::Button { button: HardwareButton::VolumeUp, action: ButtonAction::Press })
        );
        assert_eq!(
            parse(r#"{"t":"key","usage":40,"down":true}"#).input(),
            Some(InputEvent::Key { usage: 40, down: true })
        );
        assert_eq!(parse(r#"{"t":"text","text":"hi"}"#).input(), Some(InputEvent::Text("hi".into())));
        assert_eq!(parse(r#"{"t":"ping"}"#).input(), None);
        assert!(matches!(parse(r#"{"t":"auth","token":"x"}"#), ClientMessage::Auth { .. }));
        assert!(serde_json::from_str::<ClientMessage>(r#"{"t":"touch","phase":"hover","x":1,"y":1}"#).is_err());
    }

    #[test]
    fn stopping_an_unknown_stream_is_no_error() {
        stop("str_00000000000000000000000000");
        assert!(lock().get("str_00000000000000000000000000").is_none());
    }
}
