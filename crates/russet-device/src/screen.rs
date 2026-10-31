// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Screenshots, the live mirror and HID input (W10; device.md §6.8).
//!
//! Screenshots are PNGs of the whole screen from CoreDevice's screen capture service, or from the DVT screenshot
//! service behind the developer hub when that cannot be opened (device.md §6.8.1; about 6 and 7.5 frames/s). A
//! poll mirror asks for frame after frame, so each device keeps its service open in a task of its own, which
//! closes after [`IDLE`] without a request or at the first error (the next request opens a fresh one). Every
//! step has a [`STEP`] limit, so a silent device fails a request with the step's name instead of hanging it.
//!
//! The live mirror is CoreDevice's display service (device.md §6.8.2): the device encodes its screen as HEVC and
//! sends it as plaintext RTP to a UDP port inside the tunnel; [`mirror`] reassembles access units in Annex-B form
//! for a viewer to decode, and answers the encoder's feedback the way Device Hub does. While the session runs, the
//! device accepts HID input: touch through the universal HID service, buttons and keys through Indigo.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use idevice::core_device::{
    ButtonState, CallInfoBlob, DisplayServiceClient, HevcDepacketizer, ImageFormat as CaptureFormat, IndigoHidClient,
    ReportBlock, RtpPacket, ScreenCaptureServiceClient, TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE,
    UniversalHidServiceClient, build_frame_ack, build_keyframe_request, build_liveness, build_rctl,
    build_screen_audio_offer, build_screen_video_offer, build_start_audio_parameters, build_start_video_parameters,
    is_rtcp,
};
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::dvt::screenshot::ScreenshotClient;
use idevice::{IdeviceError, ReadWrite, RsdService as _};
use russet_core::Result;
use russet_core::error::codes;
use tokio::sync::{mpsc, oneshot};

use crate::coredevice::service;
use crate::error::{device_error, from_idevice};
use crate::io::ServiceConnector;
use crate::manager::DeviceHandle;
use crate::model::Udid;
use crate::tunnel::TunnelUdp;

/// A screenshot (PNG) and its size in pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Mirror options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirrorOptions {
    /// Also open the HID services the stream authenticates, so [`MirrorHandle::input`] reaches the device.
    pub input: bool,
}

/// One frame of the screen: an HEVC access unit in Annex-B form; the parameter sets precede every keyframe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessUnit {
    pub data: Vec<u8>,
    /// Holds a random-access picture (IDR, CRA or BLA), where a decoder can start.
    pub key: bool,
    /// The presentation time on the stream's 90 kHz clock, counted from the session's first frame.
    pub pts_micros: u64,
}

/// A running display session: the screen's frames and, while it lasts, the device's HID. Dropping it stops the
/// media stream.
pub struct MirrorHandle {
    pub frames: mpsc::Receiver<AccessUnit>,
    input: Option<mpsc::Sender<InputEvent>>,
    _stop: oneshot::Sender<()>,
}

impl std::fmt::Debug for MirrorHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MirrorHandle").field("input", &self.input.is_some()).finish_non_exhaustive()
    }
}

impl MirrorHandle {
    /// Sends one input event to the device.
    pub async fn input(&self, e: InputEvent) -> Result<()> {
        let tx = self
            .input_sender()
            .ok_or_else(|| device_error(codes::device::UNSUPPORTED, "this mirror was started without input"))?;
        tx.send(e).await.map_err(|_| device_error(codes::device::PROTOCOL, "the display session ended"))
    }

    /// The input queue, for a task that outlives a borrow of the handle; `None` without input.
    pub fn input_sender(&self) -> Option<mpsc::Sender<InputEvent>> {
        self.input.clone()
    }
}

/// A touch phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

/// A hardware button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareButton {
    Home,
    Lock,
    VolumeUp,
    VolumeDown,
    Mute,
    Siri,
}

/// What a button does: a press (down, its usual hold, up), or one edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonAction {
    Press,
    Down,
    Up,
}

/// One HID input event. Touch coordinates are the touchscreen's normalized 16-bit space, portrait.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    Touch {
        phase: TouchPhase,
        x: u16,
        y: u16,
    },
    Button {
        button: HardwareButton,
        action: ButtonAction,
    },
    /// A HID keyboard usage (page 0x07) going down or up.
    Key {
        usage: u16,
        down: bool,
    },
    /// Typed as key presses; characters without a US-keyboard usage are skipped.
    Text(String),
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

/// How many frames may wait for a slow viewer; past that, frames are dropped until the next keyframe.
const FRAME_QUEUE: usize = 4;

/// What Device Hub tells the display service it supports (device.md §6.8.2). Sending the device's own, larger
/// mask makes the negotiator produce an invalid video configuration.
const CLIENT_SUPPORTED_FEATURES: u64 = 140;

/// The device's video sender port, until its first packet names it.
const VIDEO_SENDER_PORT: u16 = 50001;

/// Receiver feedback, as Device Hub sends it: an RCTL report about every 50 ms (the encoder manages its references
/// and rate with it; without it fast motion corrupts the picture), a receiver report every second.
const RCTL_EVERY: Duration = Duration::from_millis(50);
const REPORT_EVERY: Duration = Duration::from_secs(1);
/// At most one keyframe request this often, while frames are being dropped.
const KEYFRAME_REQUEST_EVERY: Duration = Duration::from_millis(500);
const CNAME: &str = "russet";

/// HID keyboard usage of Left Shift.
const KEY_LEFT_SHIFT: u64 = 0xE1;

/// An access unit delimiter in the depacketizer's output, which starts every access unit after the first.
const AUD: [u8; 7] = [0, 0, 0, 1, 0x46, 0x01, 0x50];

/// A display session of `h` (device.md §6.8.2): the display service's audio stream, which establishes the
/// screen-sharing session, then its video stream on the same session; with `input`, the HID services it
/// authenticates. iOS shows its screen-sharing indicator while the session runs.
pub(crate) async fn mirror(h: &DeviceHandle, o: MirrorOptions) -> Result<MirrorHandle> {
    let conn = h.conn().clone();
    let udid = h.udid().clone();
    let stream = timed("opening the display service", conn.rsd_service(&udid, service::DISPLAY)).await?;
    let mut client = step("starting the display service", DisplayServiceClient::from_stream(stream)).await?;
    let audio = timed("binding the audio socket", conn.tunnel_udp(&udid)).await?;
    let video = timed("binding the video socket", conn.tunnel_udp(&udid)).await?;
    let ssrc = timed("negotiating the screen stream", negotiate(&mut client, &audio, &video)).await?;
    tracing::info!("display session started");
    let (frames_tx, frames) = mpsc::channel(FRAME_QUEUE);
    let (input, input_rx) = match o.input {
        true => {
            let (tx, rx) = mpsc::channel(64);
            (Some(tx), Some(rx))
        }
        false => (None, None),
    };
    if let Some(events) = input_rx {
        tokio::spawn(run_input(conn, udid, events));
    }
    let (stop, stop_rx) = oneshot::channel();
    tokio::spawn(run_display(Display { client, audio, video, ssrc }, frames_tx, stop_rx));
    Ok(MirrorHandle { frames, input, _stop: stop })
}

/// Starts the audio stream, then the video stream of display 1 on the same session (as Device Hub does). Answers
/// the SSRC the video offer declared for this receiver, which its RTCP must carry.
async fn negotiate(
    client: &mut DisplayServiceClient<Box<dyn ReadWrite>>,
    audio: &TunnelUdp,
    video: &TunnelUdp,
) -> Result<u32> {
    let failed = |what: &str, e: IdeviceError| {
        device_error(codes::device::PROTOCOL, format!("cannot build the {what} offer: {e:?}"))
    };
    let receiver = video.host_ip.to_string();
    let sender = video.peer_ip.to_string();
    // The endpoint description of a Device Hub offer the device accepted.
    let call_info = CallInfoBlob {
        call_id: 0,
        client_version: 1,
        device_type: "Mac17,7".into(),
        framework_version: "2205.3.1".into(),
        os_version: "25F71".into(),
        device_name: None,
        audio_device_uid: None,
    };
    let session = uuid::Uuid::new_v4();
    let call = || uuid::Uuid::new_v4().to_string().to_uppercase();
    let offer = build_screen_audio_offer(&call(), &call_info).map_err(|e| failed("audio", e))?;
    let params = build_start_audio_parameters(
        &receiver,
        audio.socket.local_port(),
        &sender,
        50000,
        offer,
        CLIENT_SUPPORTED_FEATURES,
        session,
    );
    step("starting the audio stream", client.start_media_stream(params)).await?;
    let mut ssrc = [0u8; 4];
    russet_core::time::os_fill(&mut ssrc);
    let ssrc = u32::from_le_bytes(ssrc);
    let offer = build_screen_video_offer(&call(), &call_info, ssrc).map_err(|e| failed("video", e))?;
    let params = build_start_video_parameters(
        &receiver,
        video.socket.local_port(),
        &sender,
        VIDEO_SENDER_PORT,
        offer,
        CLIENT_SUPPORTED_FEATURES,
        1,
        session,
    );
    step("starting the video stream", client.start_media_stream(params)).await?;
    Ok(ssrc)
}

/// An open display session's parts, owned by its task.
struct Display {
    client: DisplayServiceClient<Box<dyn ReadWrite>>,
    audio: TunnelUdp,
    video: TunnelUdp,
    ssrc: u32,
}

/// The session task: RTP in, access units out, feedback to the encoder, until stopped or the viewer leaves.
async fn run_display(mut d: Display, frames: mpsc::Sender<AccessUnit>, mut stop: oneshot::Receiver<()>) {
    let mut rx = VideoReceiver::new(d.ssrc);
    let mut rctl = tokio::time::interval(RCTL_EVERY);
    rctl.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut reports = tokio::time::interval(REPORT_EVERY);
    reports.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut waiting_for_key = false;
    let mut last_request: Option<tokio::time::Instant> = None;
    loop {
        let mut feedback = Vec::new();
        tokio::select! {
            _ = &mut stop => break,
            datagram = d.video.socket.recv() => {
                let Ok(datagram) = datagram else { break };
                for (unit, timestamp) in rx.datagram(&datagram.data, datagram.source_port) {
                    feedback.push(build_frame_ack(rx.ssrc, timestamp));
                    if waiting_for_key && !unit.key {
                        continue;
                    }
                    match frames.try_send(unit) {
                        Ok(()) => waiting_for_key = false,
                        Err(mpsc::error::TrySendError::Full(_)) => waiting_for_key = true,
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            tracing::debug!("the mirror viewer left");
                            break;
                        }
                    }
                }
                if frames.is_closed() {
                    break;
                }
                let due = last_request.is_none_or(|t| t.elapsed() >= KEYFRAME_REQUEST_EVERY);
                if waiting_for_key && due && let Some(request) = rx.keyframe_request() {
                    feedback.push(request);
                    last_request = Some(tokio::time::Instant::now());
                }
            }
            _ = rctl.tick() => feedback.extend(rx.rctl()),
            _ = reports.tick() => feedback.extend(rx.report()),
            // Read and drop the audio: unread datagrams would pile up in the tunnel's stack.
            _ = d.audio.socket.recv() => {}
        }
        for packet in feedback {
            if let Err(e) = d.video.socket.send_to(rx.sender_port, packet).await {
                tracing::debug!(error = %e, "sending receiver feedback failed");
            }
        }
    }
    if let Err(e) = d.client.stop_media_stream().await {
        tracing::debug!(error = ?e, "stopping the display session failed");
    }
    tracing::info!("display session stopped");
}

/// The input task: events in order, each awaited (a button press holds for its duration), until the handle drops.
async fn run_input(conn: Arc<dyn ServiceConnector>, udid: Udid, mut events: mpsc::Receiver<InputEvent>) {
    let mut hid = Hid::open(&conn, &udid).await;
    while let Some(event) = events.recv().await {
        hid.apply(event).await;
    }
}

/// The receiving end of the video stream: RTP in, access units and receiver feedback out.
struct VideoReceiver {
    /// Ours, as the offer declared it.
    ssrc: u32,
    /// The device's, from its packets.
    media_ssrc: Option<u32>,
    sender_port: u16,
    depacketizer: HevcDepacketizer,
    /// Depacketized output that does not yet end in a complete access unit.
    pending: Vec<u8>,
    /// Whether a packet of `pending` arrived out of sequence: the depacketizer may still hold the marker packet's
    /// payload back, so the access unit ends at the next delimiter instead of at the marker.
    gap: bool,
    first_seq: Option<u16>,
    last_seq: Option<u16>,
    /// How often the sequence number wrapped (RFC 3550's extended highest sequence number).
    cycles: u32,
    last_timestamp: Option<u32>,
    frames: u16,
    fir_seq: u8,
    clock: Clock,
    started: std::time::Instant,
}

impl VideoReceiver {
    fn new(ssrc: u32) -> Self {
        Self {
            ssrc,
            media_ssrc: None,
            sender_port: VIDEO_SENDER_PORT,
            depacketizer: HevcDepacketizer::new(),
            pending: Vec::new(),
            gap: false,
            first_seq: None,
            last_seq: None,
            cycles: 0,
            last_timestamp: None,
            frames: 0,
            fir_seq: 0,
            clock: Clock::default(),
            started: std::time::Instant::now(),
        }
    }

    /// One datagram: the access units it completes, each with its RTP timestamp. RTCP from the device is ignored.
    fn datagram(&mut self, data: &[u8], source_port: u16) -> Vec<(AccessUnit, u32)> {
        if is_rtcp(data) {
            return Vec::new();
        }
        let Some(packet) = RtpPacket::parse(data) else { return Vec::new() };
        self.sender_port = source_port;
        self.media_ssrc = Some(packet.ssrc);
        let seq = packet.sequence_number;
        self.first_seq.get_or_insert(seq);
        match self.last_seq {
            None => self.last_seq = Some(seq),
            Some(last) => {
                let ahead = seq.wrapping_sub(last);
                self.gap |= ahead != 1;
                if ahead != 0 && ahead < 0x8000 {
                    self.cycles += u32::from(seq < last);
                    self.last_seq = Some(seq);
                }
            }
        }
        self.depacketizer.push(seq, packet.timestamp, packet.payload);
        self.pending.extend(self.depacketizer.take_output());
        let mut units = Vec::new();
        // Everything before a delimiter is complete, even when the previous marker packet was lost.
        while let Some(at) = find(&self.pending[1..], &AUD).map(|i| i + 1) {
            let rest = self.pending.split_off(at);
            let unit = std::mem::replace(&mut self.pending, rest);
            let timestamp = self.last_timestamp.unwrap_or(packet.timestamp);
            units.push(self.unit(unit, timestamp));
            self.gap = false;
        }
        // The marker bit ends an access unit (RFC 7798 §4.1).
        if packet.marker && !self.gap && !self.pending.is_empty() {
            let unit = std::mem::take(&mut self.pending);
            units.push(self.unit(unit, packet.timestamp));
        }
        self.last_timestamp = Some(packet.timestamp);
        units
    }

    fn unit(&mut self, data: Vec<u8>, timestamp: u32) -> (AccessUnit, u32) {
        self.frames = self.frames.wrapping_add(1);
        let key = has_random_access(&data);
        (AccessUnit { data, key, pts_micros: self.clock.micros(timestamp) }, timestamp)
    }

    fn highest_seq(&self) -> u32 {
        (self.cycles << 16) | u32::from(self.last_seq.unwrap_or(0))
    }

    fn blocks(&self) -> Vec<ReportBlock> {
        let source_ssrc = self.media_ssrc.unwrap_or(0);
        vec![ReportBlock { source_ssrc, highest_seq: self.highest_seq(), ..ReportBlock::default() }]
    }

    fn rctl(&self) -> Option<Vec<u8>> {
        let first = self.first_seq?;
        let relative = self.last_seq.unwrap_or(first).wrapping_sub(first);
        let clock = self.started.elapsed().as_millis() as u16;
        Some(build_rctl(self.ssrc, clock, self.frames, relative))
    }

    fn report(&self) -> Option<Vec<u8>> {
        self.media_ssrc?;
        Some(build_liveness(self.ssrc, CNAME, &self.blocks()))
    }

    fn keyframe_request(&mut self) -> Option<Vec<u8>> {
        let media = self.media_ssrc?;
        self.fir_seq = self.fir_seq.wrapping_add(1);
        Some(build_keyframe_request(self.ssrc, CNAME, media, &self.blocks(), self.fir_seq))
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The HID services a display session authenticates: touch through the universal HID service, buttons and keys
/// through Indigo. Either may be missing; its events are then dropped (and logged).
struct Hid {
    touch: Option<UniversalHidServiceClient<Box<dyn ReadWrite>>>,
    keys: Option<IndigoHidClient<Box<dyn ReadWrite>>>,
}

impl Hid {
    async fn open(conn: &Arc<dyn ServiceConnector>, udid: &Udid) -> Self {
        // backboardd re-matches its HID surfaces against the new stream first (idevice's hid tool waits as long).
        tokio::time::sleep(Duration::from_millis(300)).await;
        let touch = match timed("opening touch input", conn.rsd_service(udid, service::HID_UNIVERSAL)).await {
            Ok(s) => step("starting touch input", UniversalHidServiceClient::from_stream(s)).await,
            Err(e) => Err(e),
        };
        let keys = match timed("opening button input", conn.rsd_service(udid, service::HID_INDIGO)).await {
            Ok(s) => step("starting button input", IndigoHidClient::from_stream(s)).await,
            Err(e) => Err(e),
        };
        for e in [touch.as_ref().err(), keys.as_ref().err()].into_iter().flatten() {
            tracing::warn!(reason = %e.message(), "a mirror input service is unavailable");
        }
        Self { touch: touch.ok(), keys: keys.ok() }
    }

    async fn apply(&mut self, e: InputEvent) {
        let done = match (e, self.touch.as_mut(), self.keys.as_mut()) {
            (InputEvent::Touch { phase, x, y }, Some(touch), _) => {
                let state = if phase == TouchPhase::Up { TOUCHSCREEN_STATE_RELEASE } else { TOUCHSCREEN_STATE_CONTACT };
                touch.send_touchscreen(state, x, y, None).await
            }
            (InputEvent::Button { button, action }, _, Some(keys)) => press(keys, button, action).await,
            (InputEvent::Key { usage, down }, _, Some(keys)) => {
                keys.send_keyboard(u64::from(usage), if down { ButtonState::Down } else { ButtonState::Up }).await
            }
            (InputEvent::Text(text), _, Some(keys)) => type_text(keys, &text).await,
            _ => return,
        };
        if let Err(e) = done {
            tracing::debug!(error = ?e, "an input event failed");
        }
    }
}

/// A hardware button's HID consumer-page usage and the hold of a press, as idevice's hid tool presses them.
fn button_usage(b: HardwareButton) -> (u64, u64, u64) {
    match b {
        HardwareButton::Home => (0x0C, 0x40, 80),
        HardwareButton::Lock => (0x0C, 0x30, 200),
        HardwareButton::VolumeUp => (0x0C, 0xE9, 80),
        HardwareButton::VolumeDown => (0x0C, 0xEA, 80),
        HardwareButton::Mute => (0x0C, 0xE2, 80),
        HardwareButton::Siri => (0x0C, 0xCF, 1200),
    }
}

async fn press(
    keys: &mut IndigoHidClient<Box<dyn ReadWrite>>,
    button: HardwareButton,
    action: ButtonAction,
) -> std::result::Result<(), IdeviceError> {
    let (page, code, hold) = button_usage(button);
    if action != ButtonAction::Up {
        keys.send_button(page, code, ButtonState::Down).await?;
    }
    if action == ButtonAction::Press {
        tokio::time::sleep(Duration::from_millis(hold)).await;
    }
    if action != ButtonAction::Down {
        keys.send_button(page, code, ButtonState::Up).await?;
    }
    Ok(())
}

async fn type_text(
    keys: &mut IndigoHidClient<Box<dyn ReadWrite>>,
    text: &str,
) -> std::result::Result<(), IdeviceError> {
    for (usage, shift) in text.chars().filter_map(key_usage) {
        if shift {
            keys.send_keyboard(KEY_LEFT_SHIFT, ButtonState::Down).await?;
        }
        keys.send_keyboard(usage, ButtonState::Down).await?;
        keys.send_keyboard(usage, ButtonState::Up).await?;
        if shift {
            keys.send_keyboard(KEY_LEFT_SHIFT, ButtonState::Up).await?;
        }
    }
    Ok(())
}

/// The US-keyboard HID usage of a character and whether it needs Shift (the HID Usage Tables' keyboard page).
fn key_usage(c: char) -> Option<(u64, bool)> {
    const PLAIN: &str = "-=[]\\\0;'`,./";
    const SHIFTED: &str = "_+{}|\0:\"~<>?";
    Some(match c {
        'a'..='z' => (0x04 + (c as u64 - 'a' as u64), false),
        'A'..='Z' => (0x04 + (c as u64 - 'A' as u64), true),
        '1'..='9' => (0x1E + (c as u64 - '1' as u64), false),
        '0' => (0x27, false),
        '\n' => (0x28, false),
        '\t' => (0x2B, false),
        ' ' => (0x2C, false),
        '\0' => return None,
        _ => {
            if let Some(i) = "!@#$%^&*()".find(c) {
                (0x1E + i as u64, true)
            } else if let Some(i) = PLAIN.find(c) {
                (0x2D + i as u64, false)
            } else {
                (0x2D + SHIFTED.find(c)? as u64, true)
            }
        }
    })
}

/// Whether an Annex-B access unit holds a random-access picture (HEVC NAL types 16–21: BLA, IDR, CRA).
fn has_random_access(unit: &[u8]) -> bool {
    let mut i = 0;
    while i + 3 < unit.len() {
        if unit[i] == 0 && unit[i + 1] == 0 && unit[i + 2] == 1 {
            if (16..=21).contains(&((unit[i + 3] >> 1) & 0x3f)) {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

/// RTP's 90 kHz clock as microseconds from the first frame.
#[derive(Default)]
struct Clock {
    first: Option<u32>,
}

impl Clock {
    fn micros(&mut self, timestamp: u32) -> u64 {
        let first = *self.first.get_or_insert(timestamp);
        u64::from(timestamp.wrapping_sub(first)) * 100 / 9
    }
}

/// What a decoder needs to know of an HEVC stream, from the SPS that precedes a keyframe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HevcConfig {
    /// The RFC 6381 codec string (ISO/IEC 14496-15 Annex E), for example `hvc1.1.6.L153.B0`.
    pub codec: String,
    pub coded_width: u32,
    pub coded_height: u32,
    /// The picture after the SPS's conformance window: the screen's size in pixels.
    pub width: u32,
    pub height: u32,
}

/// The [`HevcConfig`] of an Annex-B access unit that carries an SPS (every keyframe of a display session does).
pub fn hevc_config(unit: &[u8]) -> Option<HevcConfig> {
    const SPS: u8 = 33;
    let sps = nal_units(unit).find(|n| n.len() > 2 && (n[0] >> 1) & 0x3f == SPS)?;
    let rbsp = unescape(&sps[2..]);
    let mut r = Bits { data: &rbsp, at: 0 };
    r.skip(4); // sps_video_parameter_set_id
    let sub_layers = r.read(3)? as usize; // sps_max_sub_layers_minus1
    r.skip(1); // sps_temporal_id_nesting_flag
    // profile_tier_level(1, sps_max_sub_layers_minus1) (H.265 §7.3.3)
    let profile_space = r.read(2)?;
    let tier = r.read(1)?;
    let profile = r.read(5)?;
    let compatibility = r.read(32)?;
    let mut constraints = [0u8; 6];
    for c in &mut constraints {
        *c = r.read(8)? as u8;
    }
    let level = r.read(8)?;
    let mut present = [(false, false); 8];
    for p in present.iter_mut().take(sub_layers) {
        *p = (r.read(1)? == 1, r.read(1)? == 1);
    }
    if sub_layers > 0 {
        r.skip(2 * (8 - sub_layers));
    }
    for (profile_present, level_present) in present.into_iter().take(sub_layers) {
        r.skip(if profile_present { 88 } else { 0 } + if level_present { 8 } else { 0 });
    }
    r.ue()?; // sps_seq_parameter_set_id
    let chroma = r.ue()?;
    if chroma == 3 {
        r.skip(1); // separate_colour_plane_flag
    }
    let coded_width = r.ue()?;
    let coded_height = r.ue()?;
    let (mut width, mut height) = (coded_width, coded_height);
    if r.read(1)? == 1 {
        let (sub_width, sub_height) = match chroma {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        };
        let (left, right, top, bottom) = (r.ue()?, r.ue()?, r.ue()?, r.ue()?);
        width = width.checked_sub(sub_width * (left + right))?;
        height = height.checked_sub(sub_height * (top + bottom))?;
    }
    let space = ["", "A", "B", "C"][profile_space as usize];
    let tier = if tier == 1 { 'H' } else { 'L' };
    let mut codec = format!("hvc1.{space}{profile}.{:X}.{tier}{level}", compatibility.reverse_bits());
    let used = constraints.iter().rposition(|&c| c != 0).map_or(0, |i| i + 1);
    for c in &constraints[..used] {
        codec.push_str(&format!(".{c:X}"));
    }
    Some(HevcConfig { codec, coded_width, coded_height, width, height })
}

/// The NAL units of an Annex-B stream, without their start codes.
fn nal_units(stream: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 2 < stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|&s| {
            // A unit ends before the next start code, and before the leading zero of a 4-byte one.
            let end = s - 3;
            if stream[end - 1] == 0 { end - 1 } else { end }
        })
        .chain(std::iter::once(stream.len()))
        .collect();
    starts.into_iter().zip(ends).map(move |(s, e)| &stream[s..e.max(s)])
}

/// A NAL unit's payload without its emulation prevention bytes (H.265 §7.4.2).
fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// A big-endian bit reader with Exp-Golomb codes.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.at / 8)?;
            v = (v << 1) | u32::from((byte >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Some(v)
    }

    fn skip(&mut self, n: usize) {
        self.at += n;
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.read(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.read(zeros)?)
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

    /// One RTP datagram of the display stream (payload type 100).
    fn rtp(seq: u16, timestamp: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0x80, (u8::from(marker) << 7) | 100];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&timestamp.to_be_bytes());
        p.extend_from_slice(&0x1234_5678u32.to_be_bytes());
        p.extend_from_slice(payload);
        p
    }

    const VPS: &[u8] = &[0x40, 0x01, 0xAA];
    const SPS: &[u8] = &[0x42, 0x01, 0xBB];
    const PPS: &[u8] = &[0x44, 0x01, 0xCC];
    const IDR: &[u8] = &[0x26, 0x01, 0xDD, 0xDE];
    const TRAIL: &[u8] = &[0x02, 0x01, 0xEE];

    #[test]
    fn access_units_end_at_the_marker_and_keyframes_are_flagged() {
        let mut rx = VideoReceiver::new(7);
        for (seq, nal) in [VPS, SPS, PPS].into_iter().enumerate() {
            assert!(rx.datagram(&rtp(seq as u16, 9000, false, nal), 50001).is_empty());
        }
        let units = rx.datagram(&rtp(3, 9000, true, IDR), 50001);
        assert_eq!(units.len(), 1);
        let (key, ts) = &units[0];
        assert!(key.key && *ts == 9000 && key.pts_micros == 0);
        assert!(key.data.ends_with(IDR) && find(&key.data, SPS).is_some());
        let units = rx.datagram(&rtp(4, 9000 + 1500, true, TRAIL), 50001);
        let (delta, _) = &units[0];
        assert!(!delta.key && delta.data.starts_with(&AUD) && delta.data.ends_with(TRAIL));
        assert_eq!(delta.pts_micros, 16_666, "1500 ticks of 90 kHz");
        assert_eq!(rx.media_ssrc, Some(0x1234_5678));
        assert_eq!(rx.frames, 2);
    }

    #[test]
    fn a_gap_defers_the_access_unit_to_the_next_delimiter() {
        let mut rx = VideoReceiver::new(7);
        assert!(rx.datagram(&rtp(10, 100, false, IDR), 50001).is_empty());
        // seq 11 is late: the marker packet waits in the depacketizer, so nothing ends yet.
        assert!(rx.datagram(&rtp(12, 100, true, TRAIL), 50001).is_empty());
        assert!(rx.datagram(&rtp(11, 100, false, TRAIL), 50001).is_empty());
        let units = rx.datagram(&rtp(13, 200, true, TRAIL), 50001);
        assert_eq!(units.len(), 2, "the delayed unit, then the next one at its marker");
        assert_eq!(units[0].1, 100);
        assert!(units[1].0.data.starts_with(&AUD));
    }

    #[test]
    fn rtcp_and_noise_are_ignored_and_feedback_waits_for_the_stream() {
        let mut rx = VideoReceiver::new(7);
        assert!(rx.rctl().is_none() && rx.report().is_none() && rx.keyframe_request().is_none());
        assert!(rx.datagram(&[0x80, 200, 0, 6, 0, 0, 0, 1], 50001).is_empty(), "a sender report");
        assert!(rx.datagram(b"junk", 50001).is_empty());
        rx.datagram(&rtp(0xFFFF, 1, false, TRAIL), 50003);
        rx.datagram(&rtp(0, 1, true, TRAIL), 50003);
        assert_eq!(rx.sender_port, 50003);
        assert_eq!(rx.highest_seq(), 0x1_0000, "the extended sequence number counts the wrap");
        assert!(rx.rctl().is_some() && rx.report().is_some() && rx.keyframe_request().is_some());
    }

    #[test]
    fn characters_map_to_us_keyboard_usages() {
        assert_eq!(key_usage('a'), Some((0x04, false)));
        assert_eq!(key_usage('Z'), Some((0x1D, true)));
        assert_eq!(key_usage('0'), Some((0x27, false)));
        assert_eq!(key_usage('!'), Some((0x1E, true)));
        assert_eq!(key_usage(')'), Some((0x27, true)));
        assert_eq!(key_usage(';'), Some((0x33, false)));
        assert_eq!(key_usage('"'), Some((0x34, true)));
        assert_eq!(key_usage('?'), Some((0x38, true)));
        assert_eq!(key_usage('\u{e9}'), None);
        assert_eq!(key_usage('\0'), None);
    }

    /// Writes bits MSB first, Exp-Golomb codes and emulation prevention, to build an SPS.
    #[derive(Default)]
    struct Writer {
        bits: Vec<bool>,
    }

    impl Writer {
        fn put(&mut self, v: u64, n: usize) -> &mut Self {
            self.bits.extend((0..n).rev().map(|i| (v >> i) & 1 == 1));
            self
        }

        fn ue(&mut self, v: u32) -> &mut Self {
            let n = 64 - u64::from(v + 1).leading_zeros() as usize;
            self.put(0, n - 1).put(u64::from(v + 1), n)
        }

        fn nal(&self, header: [u8; 2]) -> Vec<u8> {
            let mut bits = self.bits.clone();
            bits.push(true); // rbsp_stop_one_bit
            bits.resize(bits.len().div_ceil(8) * 8, false);
            let rbsp = bits.chunks(8).map(|c| c.iter().fold(0u8, |b, &bit| (b << 1) | u8::from(bit)));
            let mut out = header.to_vec();
            let mut zeros = 0;
            for b in rbsp {
                if zeros >= 2 && b <= 3 {
                    out.push(3);
                    zeros = 0;
                }
                zeros = if b == 0 { zeros + 1 } else { 0 };
                out.push(b);
            }
            out
        }
    }

    #[test]
    fn the_sps_gives_the_codec_string_and_the_screen_size() {
        let mut w = Writer::default();
        w.put(0, 4).put(0, 3).put(1, 1); // VPS id, one sub-layer, nesting
        w.put(0, 2).put(0, 1).put(1, 5).put(0x6000_0000, 32); // Main profile, compatible with 1 and 2
        w.put(0xB0, 8).put(0, 40).put(153, 8); // progressive, frame-only; level 5.1
        w.ue(0).ue(1).ue(1208).ue(2624); // SPS id, 4:2:0, coded size
        w.put(1, 1).ue(0).ue(1).ue(0).ue(1); // conformance window: 2 px right, 2 px bottom
        let sps = w.nal([0x42, 0x01]);
        assert!(find(&sps, &[0, 0, 3]).is_some(), "the test SPS exercises emulation prevention");
        let mut unit = vec![0, 0, 0, 1];
        unit.extend_from_slice(VPS);
        unit.extend_from_slice(&[0, 0, 0, 1]);
        unit.extend_from_slice(&sps);
        unit.extend_from_slice(&[0, 0, 1]);
        unit.extend_from_slice(IDR);
        let config = hevc_config(&unit).expect("config");
        assert_eq!(config.codec, "hvc1.1.6.L153.B0");
        assert_eq!((config.coded_width, config.coded_height), (1208, 2624));
        assert_eq!((config.width, config.height), (1206, 2622));
        assert!(has_random_access(&unit));
        assert!(hevc_config(&unit[..12]).is_none(), "no SPS");
    }

    #[test]
    fn nal_units_split_on_three_and_four_byte_start_codes() {
        let stream = [0, 0, 0, 1, 0x40, 1, 0, 0, 1, 0x42, 1, 9, 0, 0, 0, 1, 0x26, 1];
        let units: Vec<&[u8]> = nal_units(&stream).collect();
        assert_eq!(units, vec![&[0x40, 1][..], &[0x42, 1, 9][..], &[0x26, 1][..]]);
        assert!(!has_random_access(&[0, 0, 1, 0x02, 1]));
    }
}
