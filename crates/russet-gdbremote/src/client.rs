// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`GdbClient`]: an async gdb-remote client over any byte stream (RD §6.2, §17.1).
//!
//! The client is strictly request/response (all-stop mode): one request in flight, except [`GdbClient::pipeline`],
//! which writes several packets in one syscall and then reads one reply per packet, in order. `O` output packets
//! that arrive while a reply is awaited are queued and come out of [`GdbClient::wait_stop`] as
//! [`StopEvent::Output`]. [`GdbClient::into_parts`] hands the stream and any bytes already read to a relay.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;
use tokio_util::codec::Decoder;

use crate::codec::{self, Frame, GdbCodec, INTERRUPT};
use crate::stop::{self, StopEvent, StopReply};
use crate::{GdbError, remote_error};

/// The request timeout when none is given (LLDB's `packet-timeout` over USB is 30 s, RD §8.4).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How many times a packet is resent after a `-` in ack mode.
const MAX_RETRANSMITS: usize = 3;

/// The largest `m`/`M` chunk, in bytes of memory, when the server did not advertise a packet size.
const DEFAULT_MEMORY_CHUNK: usize = 0x2000;

/// Handshake options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeOptions {
    /// Ask for `QStartNoAckMode` (debugserver switches after the final ack, RD §6.2).
    pub no_ack: bool,
    /// Per-packet timeout; `None` uses [`DEFAULT_TIMEOUT`].
    pub timeout: Option<Duration>,
    /// Send `qHostInfo` and fill [`ServerInfo::page_size`], `os_version` and `ostype`.
    pub host_info: bool,
}

impl Default for HandshakeOptions {
    fn default() -> Self {
        Self { no_ack: true, timeout: None, host_info: true }
    }
}

/// What the server reported during the handshake.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerInfo {
    /// `PacketSize` from `qSupported` (0 when the server did not say).
    pub packet_size: usize,
    /// `SupportedCompressions` from `qSupported`.
    pub compressions: Vec<String>,
    /// `vm-page-size` from `qHostInfo` (16384 on iOS arm64).
    pub page_size: u64,
    pub os_version: Option<String>,
    pub ostype: Option<String>,
    /// Every `qSupported` feature: `name+` → `+`, `name-` → `-`, `name=value` → `value`.
    pub features: BTreeMap<String, String>,
    /// The raw `qHostInfo` pairs.
    pub host: Vec<(String, String)>,
    /// Whether the server accepted `QThreadSuffixSupported`.
    pub thread_suffix: bool,
    /// Whether the server accepted `QStartNoAckMode`.
    pub no_ack: bool,
}

/// How to resume the inferior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeAction {
    ContinueAll,
    ContinueWithSignal {
        tid: u64,
        signal: u8,
    },
    Step {
        tid: u64,
    },
    /// Any resume packet, for example `vCont;c:-1` or `c`.
    Raw(String),
}

impl ResumeAction {
    /// The packet that performs this action.
    pub fn packet(&self) -> String {
        match self {
            Self::ContinueAll => "vCont;c".to_owned(),
            Self::ContinueWithSignal { tid, signal } => format!("vCont;C{signal:02x}:{tid:x}"),
            Self::Step { tid } => format!("vCont;s:{tid:x}"),
            Self::Raw(p) => p.clone(),
        }
    }
}

/// One loaded image (`jGetLoadedDynamicLibrariesInfos`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub load_address: u64,
    pub path: String,
    /// Upper-case UUID with dashes, as debugserver reports it.
    pub uuid: Option<String>,
    /// `mach_header.filetype` (2 = `MH_EXECUTE`, 6 = `MH_DYLIB`, 7 = `MH_DYLINKER`).
    pub filetype: Option<u32>,
    pub segments: Vec<SegmentInfo>,
}

impl ImageInfo {
    /// The image's `__TEXT` segment.
    pub fn text_segment(&self) -> Option<&SegmentInfo> {
        self.segments.iter().find(|s| s.name == "__TEXT")
    }
}

/// One segment of a loaded image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentInfo {
    pub name: String,
    pub vmaddr: u64,
    pub vmsize: u64,
    pub fileoff: u64,
    pub filesize: u64,
}

/// `jGetSharedCacheInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedCacheInfo {
    pub base_address: u64,
    pub uuid: Option<String>,
    pub no_shared_cache: bool,
}

/// `qMemoryRegionInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryRegion {
    pub start: u64,
    pub size: u64,
    /// For example `rx`; empty for unmapped space.
    pub permissions: String,
    pub name: Option<String>,
}

impl MemoryRegion {
    pub fn is_readable(&self) -> bool {
        self.permissions.contains('r')
    }
    pub fn is_executable(&self) -> bool {
        self.permissions.contains('x')
    }
}

/// `qRegisterInfo<n>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisterInfo {
    pub number: u32,
    pub name: String,
    pub alt_name: Option<String>,
    pub bitsize: u32,
    /// `generic` role: `pc`, `sp`, `fp`, `ra`, `flags`, `arg1`…
    pub generic: Option<String>,
}

/// `qProcessInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ostype: Option<String>,
    pub pairs: Vec<(String, String)>,
}

/// Counters for diagnostics (packets per JIT stop, RD §7.7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientStats {
    pub packets_sent: u64,
    pub bytes_written: u64,
    pub writes: u64,
}

/// The stream of a finished client, with everything already read from it but not consumed.
#[derive(Debug)]
pub struct RawSession<S> {
    pub stream: S,
    /// Bytes read from `stream` after the last consumed frame.
    pub buffered: BytesMut,
    /// Whether the server still expects acks.
    pub ack_mode: bool,
    pub server: ServerInfo,
}

/// An async gdb-remote client over any byte stream.
#[derive(Debug)]
pub struct GdbClient<S> {
    stream: S,
    rbuf: BytesMut,
    codec: GdbCodec,
    timeout: Duration,
    server: ServerInfo,
    output: VecDeque<Vec<u8>>,
    running: bool,
    stats: ClientStats,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + 'static> GdbClient<S> {
    /// A client without a handshake, for a connection whose state is known (`ack_mode` as the server sees it).
    pub fn raw(stream: S, ack_mode: bool) -> Self {
        Self {
            stream,
            rbuf: BytesMut::with_capacity(64 * 1024),
            codec: GdbCodec { ack_mode, ..GdbCodec::new() },
            timeout: DEFAULT_TIMEOUT,
            server: ServerInfo { no_ack: !ack_mode, ..ServerInfo::default() },
            output: VecDeque::new(),
            running: false,
            stats: ClientStats::default(),
        }
    }

    /// Resumes a session that a relay handed back ([`GdbClient::into_parts`]).
    pub fn from_parts(parts: RawSession<S>) -> Self {
        let mut c = Self::raw(parts.stream, parts.ack_mode);
        c.rbuf = parts.buffered;
        c.server = parts.server;
        c
    }

    /// The RD §6.2 handshake: `+`, `QStartNoAckMode` (when asked), `qSupported`, `QThreadSuffixSupported`,
    /// `QListThreadsInStopReply`, `QEnableErrorStrings` and `qHostInfo`.
    pub async fn handshake(stream: S, opts: HandshakeOptions) -> Result<(Self, ServerInfo), GdbError> {
        let mut c = Self::raw(stream, true);
        if let Some(t) = opts.timeout {
            c.timeout = t;
        }
        // LLDB acks first, so a stub that already sent something gets its ack.
        c.write_raw(b"+").await?;
        if opts.no_ack {
            let reply = c.request(b"QStartNoAckMode").await?;
            if &reply[..] == b"OK" {
                c.codec.ack_mode = false;
                c.server.no_ack = true;
            }
        }
        let supported = c.request(b"qSupported:swbreak+;hwbreak+;vContSupported+").await?;
        for feature in supported.split(|&b| b == b';').filter(|f| !f.is_empty()) {
            let text = String::from_utf8_lossy(feature);
            let (name, value) = match text.split_once('=') {
                Some((n, v)) => (n.to_owned(), v.to_owned()),
                None if text.ends_with('+') || text.ends_with('-') => {
                    (text[..text.len() - 1].to_owned(), text[text.len() - 1..].to_owned())
                }
                None => (text.into_owned(), String::new()),
            };
            match name.as_str() {
                "PacketSize" => {
                    c.server.packet_size = codec::parse_hex_u64(value.as_bytes()).unwrap_or(0) as usize;
                }
                "SupportedCompressions" => {
                    c.server.compressions = value.split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect();
                }
                _ => {}
            }
            c.server.features.insert(name, value);
        }
        c.server.thread_suffix = &c.request(b"QThreadSuffixSupported").await?[..] == b"OK";
        c.request(b"QListThreadsInStopReply").await?;
        c.request(b"QEnableErrorStrings").await?;
        if opts.host_info {
            let host = c.request(b"qHostInfo").await?;
            let pairs = codec::parse_kv(&host);
            for (k, v) in &pairs {
                match k.as_str() {
                    "vm-page-size" => c.server.page_size = v.parse().unwrap_or(0),
                    "os_version" => c.server.os_version = Some(v.clone()),
                    "ostype" => c.server.ostype = Some(v.clone()),
                    _ => {}
                }
            }
            c.server.host = pairs;
        }
        let info = c.server.clone();
        Ok((c, info))
    }

    /// What the handshake found.
    pub fn server(&self) -> &ServerInfo {
        &self.server
    }

    /// Changes the per-request timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    pub fn stats(&self) -> ClientStats {
        self.stats
    }

    /// Whether a resume is outstanding (no stop reply seen since the last resume).
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Marks a resume as outstanding, for a client rebuilt from a relay while the inferior runs, so that
    /// [`GdbClient::interrupt`] sends `0x03` instead of `?`.
    pub fn assume_running(&mut self) {
        self.running = true;
    }

    /// Sends one packet and returns its decoded reply. `O` output packets that arrive first are queued for
    /// [`GdbClient::wait_stop`].
    pub async fn request(&mut self, payload: &[u8]) -> Result<Bytes, GdbError> {
        self.send_packet(payload).await?;
        self.read_reply().await
    }

    /// Sends a packet whose reply must be `OK`.
    pub async fn request_ok(&mut self, payload: &[u8]) -> Result<(), GdbError> {
        let reply = self.request(payload).await?;
        check_ok(&reply)
    }

    /// Writes all packets in one syscall, then collects one reply per packet, in order. In ack mode the packets
    /// go one by one, because each needs its ack.
    pub async fn pipeline(&mut self, payloads: &[Vec<u8>]) -> Result<Vec<Bytes>, GdbError> {
        if self.codec.ack_mode {
            let mut out = Vec::with_capacity(payloads.len());
            for p in payloads {
                out.push(self.request(p).await?);
            }
            return Ok(out);
        }
        let mut buf = Vec::with_capacity(payloads.iter().map(|p| p.len() + 4).sum());
        for p in payloads {
            buf.extend_from_slice(&codec::frame(p));
        }
        self.write_raw(&buf).await?;
        self.stats.packets_sent += payloads.len() as u64;
        let mut out = Vec::with_capacity(payloads.len());
        for _ in payloads {
            out.push(self.read_reply().await?);
        }
        Ok(out)
    }

    /// `vAttach;<pid>`: the stub attaches and reports the stop (`T11` on iOS).
    pub async fn attach(&mut self, pid: u32) -> Result<StopReply, GdbError> {
        let reply = self.request(format!("vAttach;{pid:x}").as_bytes()).await?;
        expect_stopped(stop::parse_stop(&reply)?)
    }

    /// `vRun;<program>[;<arg>…]`: lldb-server launches a program stopped at its entry (host tests; debugserver on
    /// iOS 17+ cannot spawn apps).
    pub async fn run_program(&mut self, program: &str, args: &[String]) -> Result<StopReply, GdbError> {
        let mut packet = format!("vRun;{}", codec::hex_encode(program.as_bytes()));
        for a in args {
            packet.push(';');
            packet.push_str(&codec::hex_encode(a.as_bytes()));
        }
        let reply = self.request(packet.as_bytes()).await?;
        expect_stopped(stop::parse_stop(&reply)?)
    }

    /// `?`: why the process is stopped.
    pub async fn stop_reason(&mut self) -> Result<StopEvent, GdbError> {
        let reply = self.request(b"?").await?;
        stop::parse_stop(&reply)
    }

    /// Resumes without waiting for the next stop.
    pub async fn resume(&mut self, action: ResumeAction) -> Result<(), GdbError> {
        self.send_packet(action.packet().as_bytes()).await?;
        self.running = true;
        Ok(())
    }

    /// Waits for the next stop, exit or output packet (no timeout: the inferior may run for a long time).
    pub async fn wait_stop(&mut self) -> Result<StopEvent, GdbError> {
        if let Some(out) = self.output.pop_front() {
            return Ok(StopEvent::Output(out));
        }
        let payload = self.read_packet(None).await?;
        let decoded = codec::decode_payload(&payload);
        if stop::is_output_packet(&decoded) {
            return stop::parse_stop(&decoded);
        }
        if stop::is_stop_packet(&decoded) {
            self.running = false;
            return stop::parse_stop(&decoded);
        }
        if decoded.first() == Some(&b'E') {
            self.running = false;
            return Err(remote_error(&decoded));
        }
        Err(GdbError::Protocol(format!("expected a stop reply, got {:?}", String::from_utf8_lossy(&decoded))))
    }

    /// [`GdbClient::wait_stop`] with a deadline.
    pub async fn wait_stop_timeout(&mut self, limit: Duration) -> Result<StopEvent, GdbError> {
        match tokio::time::timeout(limit, self.wait_stop()).await {
            Ok(r) => r,
            Err(_) => Err(GdbError::Timeout("stop reply")),
        }
    }

    /// Stops a running inferior (`0x03`) and returns the stop; when nothing runs it asks `?` instead.
    pub async fn interrupt(&mut self) -> Result<StopReply, GdbError> {
        if !self.running {
            return expect_stopped(self.stop_reason().await?);
        }
        self.write_raw(&[INTERRUPT]).await?;
        // Output that arrives first is kept for the next wait_stop; it must not be fed back into this loop.
        let deadline = Instant::now() + self.timeout;
        let mut held = Vec::new();
        let result = loop {
            match tokio::time::timeout_at(deadline, self.wait_stop()).await {
                Err(_) => break Err(GdbError::Timeout("stop after interrupt")),
                Ok(Ok(StopEvent::Output(o))) => held.push(o),
                Ok(r) => break r.and_then(expect_stopped),
            }
        };
        self.output.extend(held);
        result
    }

    /// Reads `len` bytes at `addr` (`m`); a read that runs into unmapped memory returns the readable prefix.
    pub async fn read_memory(&mut self, addr: u64, len: usize) -> Result<Vec<u8>, GdbError> {
        let chunk = self.memory_chunk();
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let want = chunk.min(len - out.len());
            let at = addr + out.len() as u64;
            let reply = self.request(format!("m{at:x},{want:x}").as_bytes()).await?;
            if reply.first() == Some(&b'E') {
                if out.is_empty() {
                    return Err(remote_error(&reply));
                }
                break;
            }
            let bytes = codec::hex_decode(&reply)
                .ok_or_else(|| GdbError::Protocol("memory read reply is not hex".to_owned()))?;
            let got = bytes.len();
            out.extend_from_slice(&bytes);
            if got < want {
                break;
            }
        }
        Ok(out)
    }

    /// Writes `data` at `addr` (`M`).
    pub async fn write_memory(&mut self, addr: u64, data: &[u8]) -> Result<(), GdbError> {
        for (i, part) in data.chunks(self.memory_chunk()).enumerate() {
            let at = addr + (i * self.memory_chunk()) as u64;
            self.request_ok(memory_write_packet(at, part).as_bytes()).await?;
        }
        Ok(())
    }

    /// `Z0,<addr>,<kind>`: a software breakpoint (`kind` is the instruction size: 4 on arm64, 1 on x86).
    pub async fn set_breakpoint(&mut self, addr: u64, kind: u8) -> Result<(), GdbError> {
        self.request_ok(format!("Z0,{addr:x},{kind:x}").as_bytes()).await
    }

    /// `z0,<addr>,<kind>`.
    pub async fn remove_breakpoint(&mut self, addr: u64, kind: u8) -> Result<(), GdbError> {
        self.request_ok(format!("z0,{addr:x},{kind:x}").as_bytes()).await
    }

    /// `P<reg>=<value>`: eight little-endian bytes, for thread `tid`.
    pub async fn write_register(&mut self, tid: u64, reg: u32, value: u64) -> Result<(), GdbError> {
        let packet = self.register_write_packet(tid, reg, value);
        if !self.server.thread_suffix {
            self.request_ok(format!("Hg{tid:x}").as_bytes()).await?;
        }
        self.request_ok(packet.as_bytes()).await
    }

    /// `p<reg>`: a register of thread `tid`, up to eight bytes.
    pub async fn read_register(&mut self, tid: u64, reg: u32) -> Result<u64, GdbError> {
        let packet = if self.server.thread_suffix {
            format!("p{reg:x};thread:{tid:x};")
        } else {
            self.request_ok(format!("Hg{tid:x}").as_bytes()).await?;
            format!("p{reg:x}")
        };
        let reply = self.request(packet.as_bytes()).await?;
        if reply.first() == Some(&b'E') {
            return Err(remote_error(&reply));
        }
        codec::parse_le_hex_u64(&reply)
            .ok_or_else(|| GdbError::Protocol(format!("register {reg} is not a ≤ 8-byte value")))
    }

    /// The `P` packet [`GdbClient::write_register`] sends (with the thread suffix), for pipelines.
    pub fn register_write_packet(&self, tid: u64, reg: u32, value: u64) -> String {
        if self.server.thread_suffix {
            format!("P{reg:x}={};thread:{tid:x};", codec::le_hex(value, 8))
        } else {
            format!("P{reg:x}={}", codec::le_hex(value, 8))
        }
    }

    /// Every image dyld knows (`jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}`). dyld reports the
    /// main executable and dyld itself even for a suspended, not yet started process (RD V10).
    pub async fn loaded_images(&mut self) -> Result<Vec<ImageInfo>, GdbError> {
        let reply = self.json_request("jGetLoadedDynamicLibrariesInfos:", r#"{"fetch_all_solibs":true}"#).await?;
        parse_images(&reply)
    }

    /// `jGetSharedCacheInfo`.
    pub async fn shared_cache_info(&mut self) -> Result<SharedCacheInfo, GdbError> {
        let reply = self.json_request("jGetSharedCacheInfo:", "{}").await?;
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            shared_cache_base_address: u64,
            #[serde(default)]
            shared_cache_uuid: Option<String>,
            #[serde(default)]
            no_shared_cache: bool,
        }
        let raw: Raw = serde_json::from_slice(&reply)
            .map_err(|e| GdbError::Protocol(format!("jGetSharedCacheInfo reply: {e}")))?;
        Ok(SharedCacheInfo {
            base_address: raw.shared_cache_base_address,
            uuid: raw.shared_cache_uuid,
            no_shared_cache: raw.no_shared_cache,
        })
    }

    /// `qMemoryRegionInfo:<addr>`.
    pub async fn memory_region(&mut self, addr: u64) -> Result<MemoryRegion, GdbError> {
        let reply = self.request(format!("qMemoryRegionInfo:{addr:x}").as_bytes()).await?;
        if reply.first() == Some(&b'E') {
            return Err(remote_error(&reply));
        }
        if reply.is_empty() {
            return Err(GdbError::Unsupported("qMemoryRegionInfo"));
        }
        let mut region = MemoryRegion::default();
        for (k, v) in codec::parse_kv(&reply) {
            match k.as_str() {
                "start" => region.start = codec::parse_hex_u64(v.as_bytes()).unwrap_or(0),
                "size" => region.size = codec::parse_hex_u64(v.as_bytes()).unwrap_or(0),
                "permissions" => region.permissions = v,
                "name" => {
                    region.name = codec::hex_decode(v.as_bytes()).map(|b| String::from_utf8_lossy(&b).into_owned())
                }
                _ => {}
            }
        }
        Ok(region)
    }

    /// `qThreadStopInfo<tid>`.
    pub async fn thread_stop_info(&mut self, tid: u64) -> Result<StopReply, GdbError> {
        let reply = self.request(format!("qThreadStopInfo{tid:x}").as_bytes()).await?;
        expect_stopped(stop::parse_stop(&reply)?)
    }

    /// `qProcessInfo`.
    pub async fn process_info(&mut self) -> Result<ProcessInfo, GdbError> {
        let reply = self.request(b"qProcessInfo").await?;
        if reply.first() == Some(&b'E') {
            return Err(remote_error(&reply));
        }
        let pairs = codec::parse_kv(&reply);
        let mut info = ProcessInfo::default();
        for (k, v) in &pairs {
            match k.as_str() {
                "pid" => info.pid = codec::parse_hex_u64(v.as_bytes()).unwrap_or(0) as u32,
                "ostype" => info.ostype = Some(v.clone()),
                _ => {}
            }
        }
        info.pairs = pairs;
        Ok(info)
    }

    /// Every register the stub describes (`qRegisterInfo0`, `qRegisterInfo1`, … until an error).
    pub async fn register_infos(&mut self) -> Result<Vec<RegisterInfo>, GdbError> {
        let mut out = Vec::new();
        for n in 0u32..512 {
            let reply = self.request(format!("qRegisterInfo{n:x}").as_bytes()).await?;
            if reply.is_empty() || reply.first() == Some(&b'E') {
                break;
            }
            let mut info = RegisterInfo { number: n, ..RegisterInfo::default() };
            for (k, v) in codec::parse_kv(&reply) {
                match k.as_str() {
                    "name" => info.name = v,
                    "alt-name" => info.alt_name = Some(v),
                    "bitsize" => info.bitsize = v.parse().unwrap_or(0),
                    "generic" => info.generic = Some(v),
                    _ => {}
                }
            }
            out.push(info);
        }
        Ok(out)
    }

    /// `D`: detaches; the process keeps running.
    pub async fn detach(mut self) -> Result<(), GdbError> {
        self.request_ok(b"D").await
    }

    /// `k`: kills the process. Returns the exit reply when the stub sent one before closing.
    pub async fn kill(mut self) -> Result<Option<StopEvent>, GdbError> {
        if self.running {
            // An all-stop stub ignores packets while the inferior runs; stop it first.
            let _ = self.write_raw(&[INTERRUPT]).await;
            let _ = tokio::time::timeout(Duration::from_secs(2), self.wait_stop()).await;
        }
        self.send_packet(b"k").await?;
        match tokio::time::timeout(Duration::from_secs(5), self.read_packet(None)).await {
            Ok(Ok(p)) => Ok(stop::parse_stop_raw(&p).ok()),
            Ok(Err(GdbError::Disconnected)) | Err(_) => Ok(None),
            Ok(Err(e)) => Err(e),
        }
    }

    /// Hands the stream and the unread bytes to a relay (the broker's interposer).
    pub fn into_parts(self) -> RawSession<S> {
        RawSession { stream: self.stream, buffered: self.rbuf, ack_mode: self.codec.ack_mode, server: self.server }
    }

    /// Writes raw bytes (an interrupt, an ack, preframed packets) and flushes.
    pub async fn write_raw(&mut self, bytes: &[u8]) -> Result<(), GdbError> {
        self.stream.write_all(bytes).await?;
        self.stream.flush().await?;
        self.stats.bytes_written += bytes.len() as u64;
        self.stats.writes += 1;
        Ok(())
    }

    /// Sends one packet; in ack mode waits for its `+` and resends after a `-`.
    pub async fn send_packet(&mut self, payload: &[u8]) -> Result<(), GdbError> {
        let framed = codec::frame(payload);
        self.stats.packets_sent += 1;
        self.write_raw(&framed).await?;
        if !self.codec.ack_mode {
            return Ok(());
        }
        let deadline = Instant::now() + self.timeout;
        let mut resent = 0;
        loop {
            match self.next_frame(Some(deadline)).await? {
                Frame::Ack => return Ok(()),
                Frame::Nack if resent < MAX_RETRANSMITS => {
                    resent += 1;
                    self.write_raw(&framed).await?;
                }
                Frame::Nack => return Err(GdbError::Protocol("packet rejected after retransmits".to_owned())),
                // A server that sends its reply without an ack (some stubs) answered already: keep the packet.
                Frame::Packet(p) => {
                    self.requeue_packet(&p);
                    return Ok(());
                }
                Frame::Interrupt | Frame::Notification(_) | Frame::Corrupt(_) => {}
            }
        }
    }

    fn requeue_packet(&mut self, payload: &Bytes) {
        let mut framed = BytesMut::from(&codec::frame(payload)[..]);
        framed.extend_from_slice(&self.rbuf);
        self.rbuf = framed;
    }

    /// Reads the next packet that is a reply, queueing `O` packets.
    async fn read_reply(&mut self) -> Result<Bytes, GdbError> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let payload = self.read_packet(Some(deadline)).await?;
            let decoded = codec::decode_payload(&payload);
            if stop::is_output_packet(&decoded) {
                if let Some(bytes) = codec::hex_decode(&decoded[1..]) {
                    self.output.push_back(bytes);
                }
                continue;
            }
            return Ok(Bytes::from(decoded));
        }
    }

    /// Reads the next packet, acking it in ack mode and answering corrupt ones with `-`.
    async fn read_packet(&mut self, deadline: Option<Instant>) -> Result<Bytes, GdbError> {
        loop {
            match self.next_frame(deadline).await? {
                Frame::Packet(p) => {
                    if self.codec.ack_mode {
                        self.write_raw(b"+").await?;
                    }
                    return Ok(p);
                }
                Frame::Corrupt(_) => self.write_raw(b"-").await?,
                Frame::Ack | Frame::Nack | Frame::Interrupt | Frame::Notification(_) => {}
            }
        }
    }

    async fn next_frame(&mut self, deadline: Option<Instant>) -> Result<Frame, GdbError> {
        loop {
            if let Some(frame) = self.codec.decode(&mut self.rbuf)? {
                return Ok(frame);
            }
            self.rbuf.reserve(64 * 1024);
            let n = match deadline {
                Some(d) => match tokio::time::timeout_at(d, self.stream.read_buf(&mut self.rbuf)).await {
                    Ok(r) => r?,
                    Err(_) => return Err(GdbError::Timeout("reply")),
                },
                None => self.stream.read_buf(&mut self.rbuf).await?,
            };
            if n == 0 {
                return Err(GdbError::Disconnected);
            }
        }
    }

    async fn json_request(&mut self, name: &str, json: &str) -> Result<Bytes, GdbError> {
        let mut packet = name.as_bytes().to_vec();
        packet.extend_from_slice(&codec::escape_binary(json.as_bytes()));
        let reply = self.request(&packet).await?;
        if reply.is_empty() {
            return Err(GdbError::Unsupported("JSON packet not supported by this stub"));
        }
        if reply.first() == Some(&b'E') {
            return Err(remote_error(&reply));
        }
        Ok(reply)
    }

    fn memory_chunk(&self) -> usize {
        match self.server.packet_size {
            // Hex doubles the size; leave room for the header.
            n if n > 64 => ((n - 32) / 2).max(16),
            _ => DEFAULT_MEMORY_CHUNK,
        }
    }
}

/// The `M` packet that writes `data` at `addr`.
pub fn memory_write_packet(addr: u64, data: &[u8]) -> String {
    format!("M{addr:x},{:x}:{}", data.len(), codec::hex_encode(data))
}

/// `OK` is success; `Exx` is a remote error; anything else is a protocol error.
pub fn check_ok(reply: &[u8]) -> Result<(), GdbError> {
    match reply {
        b"OK" => Ok(()),
        [b'E', ..] => Err(remote_error(reply)),
        [] => Err(GdbError::Unsupported("empty reply")),
        other => Err(GdbError::Protocol(format!("expected OK, got {:?}", String::from_utf8_lossy(other)))),
    }
}

fn expect_stopped(event: StopEvent) -> Result<StopReply, GdbError> {
    match event {
        StopEvent::Stopped(r) => Ok(r),
        StopEvent::Exited { code } => Err(GdbError::Exited { code: Some(code), signal: None }),
        StopEvent::Terminated { signal } => Err(GdbError::Exited { code: None, signal: Some(signal) }),
        StopEvent::Output(_) => Err(GdbError::Protocol("output where a stop reply was expected".to_owned())),
    }
}

/// Parses a `jGetLoadedDynamicLibrariesInfos` reply.
pub fn parse_images(json: &[u8]) -> Result<Vec<ImageInfo>, GdbError> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        images: Vec<RawImage>,
    }
    #[derive(Deserialize)]
    struct RawImage {
        #[serde(default)]
        load_address: u64,
        #[serde(default)]
        pathname: String,
        #[serde(default)]
        uuid: Option<String>,
        #[serde(default)]
        mach_header: Option<RawHeader>,
        #[serde(default)]
        segments: Vec<RawSegment>,
    }
    #[derive(Deserialize)]
    struct RawHeader {
        #[serde(default)]
        filetype: Option<u32>,
    }
    #[derive(Deserialize)]
    struct RawSegment {
        #[serde(default)]
        name: String,
        #[serde(default)]
        vmaddr: u64,
        #[serde(default)]
        vmsize: u64,
        #[serde(default)]
        fileoff: u64,
        #[serde(default)]
        filesize: u64,
    }
    let reply: Reply = serde_json::from_slice(json)
        .map_err(|e| GdbError::Protocol(format!("jGetLoadedDynamicLibrariesInfos reply: {e}")))?;
    Ok(reply
        .images
        .into_iter()
        .map(|i| ImageInfo {
            load_address: i.load_address,
            path: i.pathname,
            uuid: i.uuid,
            filetype: i.mach_header.and_then(|h| h.filetype),
            segments: i
                .segments
                .into_iter()
                .map(|s| SegmentInfo {
                    name: s.name,
                    vmaddr: s.vmaddr,
                    vmsize: s.vmsize,
                    fileoff: s.fileoff,
                    filesize: s.filesize,
                })
                .collect(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};

    use super::*;

    /// A tiny scripted stub: answers each request with the next canned reply and records what it got.
    struct Script {
        replies: Vec<(&'static [u8], Vec<u8>)>,
        ack: bool,
    }

    async fn serve(mut io: DuplexStream, script: Script) -> Vec<Vec<u8>> {
        let mut seen = Vec::new();
        let mut codec = GdbCodec { ack_mode: script.ack, ..GdbCodec::new() };
        let mut buf = BytesMut::new();
        let mut replies = script.replies.into_iter();
        let mut ack = script.ack;
        loop {
            let frame = loop {
                if let Some(f) = codec.decode(&mut buf).unwrap() {
                    break Some(f);
                }
                let n = io.read_buf(&mut buf).await.unwrap();
                if n == 0 {
                    break None;
                }
            };
            let Some(frame) = frame else { return seen };
            let payload = match frame {
                Frame::Packet(p) => p,
                Frame::Interrupt => Bytes::from_static(b"\x03"),
                _ => continue,
            };
            seen.push(payload.to_vec());
            let Some((expect, reply)) = replies.next() else { return seen };
            assert!(
                payload.starts_with(expect),
                "expected {:?}, got {:?}",
                String::from_utf8_lossy(expect),
                String::from_utf8_lossy(&payload)
            );
            if ack && &payload[..] != b"\x03" {
                io.write_all(b"+").await.unwrap();
            }
            if !reply.is_empty() || expect != b"vCont" {
                io.write_all(&reply).await.unwrap();
            }
            if &payload[..] == b"QStartNoAckMode" {
                ack = false;
                codec.ack_mode = false;
            }
        }
    }

    fn pkt(p: &[u8]) -> Vec<u8> {
        codec::frame(p)
    }

    fn handshake_script(extra: Vec<(&'static [u8], Vec<u8>)>) -> Script {
        let mut replies = vec![
            (&b"QStartNoAckMode"[..], pkt(b"OK")),
            (
                &b"qSupported"[..],
                pkt(b"PacketSize=20000;qEcho+;SupportedCompressions=lzfse,zlib-deflate;vContSupported+"),
            ),
            (&b"QThreadSuffixSupported"[..], pkt(b"OK")),
            (&b"QListThreadsInStopReply"[..], pkt(b"OK")),
            (&b"QEnableErrorStrings"[..], pkt(b"OK")),
            (&b"qHostInfo"[..], pkt(b"cputype:16777228;ostype:ios;vendor:apple;os_version:27.0.0;vm-page-size:16384;")),
        ];
        replies.extend(extra);
        Script { replies, ack: true }
    }

    #[tokio::test]
    async fn handshake_switches_to_no_ack_and_reads_server_facts() {
        let (client_io, server_io) = duplex(1 << 16);
        let server = tokio::spawn(serve(server_io, handshake_script(vec![])));
        let (client, info) = GdbClient::handshake(client_io, HandshakeOptions::default()).await.unwrap();
        assert!(info.no_ack && info.thread_suffix);
        assert_eq!(info.packet_size, 0x20000);
        assert_eq!(info.compressions, vec!["lzfse".to_owned(), "zlib-deflate".to_owned()]);
        assert_eq!(info.page_size, 16384);
        assert_eq!(info.ostype.as_deref(), Some("ios"));
        assert_eq!(info.os_version.as_deref(), Some("27.0.0"));
        assert_eq!(info.features.get("qEcho").map(String::as_str), Some("+"));
        drop(client);
        let seen = server.await.unwrap();
        assert_eq!(seen.len(), 6);
    }

    #[tokio::test]
    async fn attach_breakpoints_registers_memory_and_detach() {
        let (client_io, server_io) = duplex(1 << 16);
        let script = handshake_script(vec![
            (&b"vAttach;3e8"[..], pkt(b"T11thread:1a2b;threads:1a2b;reason:signal;")),
            (&b"Z0,100004000,4"[..], pkt(b"OK")),
            (&b"P20=0080000001000000;thread:1a2b;"[..], pkt(b"OK")),
            (&b"p1e;thread:1a2b;"[..], pkt(b"a043000001000000")),
            (&b"m100008000,8"[..], pkt(b"4948454c50454421")),
            (&b"M100008000,2:0000"[..], pkt(b"OK")),
            (&b"z0,100004000,4"[..], pkt(b"OK")),
            (&b"D"[..], pkt(b"OK")),
        ]);
        let server = tokio::spawn(serve(server_io, script));
        let (mut c, _) = GdbClient::handshake(client_io, HandshakeOptions::default()).await.unwrap();
        let stop = c.attach(1000).await.unwrap();
        assert_eq!((stop.signal, stop.thread), (0x11, Some(0x1a2b)));
        c.set_breakpoint(0x1_0000_4000, 4).await.unwrap();
        c.write_register(0x1a2b, 32, 0x1_0000_8000).await.unwrap();
        assert_eq!(c.read_register(0x1a2b, 30).await.unwrap(), 0x1_0000_43a0);
        assert_eq!(c.read_memory(0x1_0000_8000, 8).await.unwrap(), b"IHELPED!");
        c.write_memory(0x1_0000_8000, &[0, 0]).await.unwrap();
        c.remove_breakpoint(0x1_0000_4000, 4).await.unwrap();
        c.detach().await.unwrap();
        assert_eq!(server.await.unwrap().len(), 14);
    }

    #[tokio::test]
    async fn pipeline_writes_once_and_reads_replies_in_order() {
        let (client_io, server_io) = duplex(1 << 16);
        let script = handshake_script(vec![
            (&b"M1000,8:"[..], pkt(b"OK")),
            (&b"M7fff,2:"[..], pkt(b"OK")),
            (&b"P20="[..], pkt(b"E09")),
        ]);
        let server = tokio::spawn(serve(server_io, script));
        let (mut c, _) = GdbClient::handshake(client_io, HandshakeOptions::default()).await.unwrap();
        let before = c.stats();
        let packets = vec![
            memory_write_packet(0x1000, b"IHELPED!").into_bytes(),
            memory_write_packet(0x7fff, &[0, 0]).into_bytes(),
            c.register_write_packet(1, 32, 0x2000).into_bytes(),
        ];
        let replies = c.pipeline(&packets).await.unwrap();
        assert_eq!(c.stats().writes - before.writes, 1, "one write for the whole batch");
        assert_eq!(&replies[0][..], b"OK");
        assert!(matches!(check_ok(&replies[2]), Err(GdbError::Remote { code: 9, .. })));
        drop(c);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn output_is_queued_and_stops_end_a_resume() {
        let (client_io, server_io) = duplex(1 << 16);
        let mut stop_with_output = pkt(b"O68690a");
        stop_with_output.extend_from_slice(&pkt(b"T05thread:1;reason:breakpoint;"));
        let script = handshake_script(vec![(&b"vCont;c"[..], stop_with_output), (&b"vCont;C0b:1"[..], pkt(b"X0b"))]);
        let server = tokio::spawn(serve(server_io, script));
        let (mut c, _) = GdbClient::handshake(client_io, HandshakeOptions::default()).await.unwrap();
        c.resume(ResumeAction::ContinueAll).await.unwrap();
        assert!(c.is_running());
        assert_eq!(c.wait_stop().await.unwrap(), StopEvent::Output(b"hi\n".to_vec()));
        match c.wait_stop().await.unwrap() {
            StopEvent::Stopped(r) => assert!(r.is_breakpoint()),
            other => panic!("{other:?}"),
        }
        assert!(!c.is_running());
        c.resume(ResumeAction::ContinueWithSignal { tid: 1, signal: 11 }).await.unwrap();
        assert_eq!(c.wait_stop().await.unwrap(), StopEvent::Terminated { signal: 11 });
        drop(c);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn interrupt_sends_the_raw_byte_and_kill_reads_the_exit() {
        let (client_io, server_io) = duplex(1 << 16);
        let script = handshake_script(vec![
            (&b"vCont;c"[..], Vec::new()),
            (&b"\x03"[..], pkt(b"T11thread:1;reason:signal;")),
            (&b"k"[..], pkt(b"X09")),
        ]);
        let server = tokio::spawn(serve(server_io, script));
        let (mut c, _) = GdbClient::handshake(client_io, HandshakeOptions::default()).await.unwrap();
        c.resume(ResumeAction::ContinueAll).await.unwrap();
        let stop = c.interrupt().await.unwrap();
        assert_eq!(stop.signal, 0x11);
        assert_eq!(c.kill().await.unwrap(), Some(StopEvent::Terminated { signal: 9 }));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_interrupt_keeps_output_that_arrives_before_the_stop() {
        let (client_io, server_io) = duplex(1 << 16);
        let mut out_then_stop = pkt(b"O6869");
        out_then_stop.extend_from_slice(&pkt(b"O7468657265"));
        out_then_stop.extend_from_slice(&pkt(b"T11thread:1;reason:signal;"));
        let script = handshake_script(vec![(&b"vCont;c"[..], Vec::new()), (&b"\x03"[..], out_then_stop)]);
        let server = tokio::spawn(serve(server_io, script));
        let (mut c, _) = GdbClient::handshake(client_io, HandshakeOptions::default()).await.unwrap();
        c.resume(ResumeAction::ContinueAll).await.unwrap();
        let stop = tokio::time::timeout(Duration::from_secs(5), c.interrupt()).await.unwrap().unwrap();
        assert_eq!(stop.signal, 0x11);
        assert_eq!(c.wait_stop().await.unwrap(), StopEvent::Output(b"hi".to_vec()));
        assert_eq!(c.wait_stop().await.unwrap(), StopEvent::Output(b"there".to_vec()));
        drop(c);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn json_packets_are_escaped_and_images_parse() {
        let (client_io, server_io) = duplex(1 << 16);
        let images = br#"{"images":[{"load_address":4294967296,"pathname":"/private/var/containers/Bundle/Application/X/RussetProbe.app/RussetProbe","uuid":"4C4C44A9-5555-3144-A1B2-60BFA66E4425","mach_header":{"filetype":2},"segments":[{"name":"__PAGEZERO","vmaddr":0,"vmsize":4294967296,"fileoff":0,"filesize":0},{"name":"__TEXT","vmaddr":4294967296,"vmsize":16384,"fileoff":0,"filesize":16384}]}]}"#;
        let script = handshake_script(vec![
            // The '{' is plain, the '}' is escaped as "}]".
            (&b"jGetLoadedDynamicLibrariesInfos:{\"fetch_all_solibs\":true}]"[..], pkt(&codec::escape_binary(images))),
            (
                &b"jGetSharedCacheInfo:{}]"[..],
                pkt(&codec::escape_binary(
                    br#"{"shared_cache_base_address":6442450944,"shared_cache_uuid":"0D1C","no_shared_cache":false}"#,
                )),
            ),
            (&b"qMemoryRegionInfo:100000000"[..], pkt(b"start:100000000;size:4000;permissions:rx;name:5f5f54455854;")),
        ]);
        let server = tokio::spawn(serve(server_io, script));
        let (mut c, _) = GdbClient::handshake(client_io, HandshakeOptions::default()).await.unwrap();
        let images = c.loaded_images().await.unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].load_address, 0x1_0000_0000);
        assert_eq!(images[0].filetype, Some(2));
        assert_eq!(images[0].text_segment().unwrap().vmsize, 16384);
        let cache = c.shared_cache_info().await.unwrap();
        assert_eq!((cache.base_address, cache.uuid.as_deref()), (0x1_8000_0000, Some("0D1C")));
        let region = c.memory_region(0x1_0000_0000).await.unwrap();
        assert!(region.is_executable() && region.is_readable());
        assert_eq!(region.name.as_deref(), Some("__TEXT"));
        drop(c);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn remote_errors_and_timeouts() {
        let (client_io, server_io) = duplex(1 << 16);
        let mut msg = b"E01;".to_vec();
        msg.extend_from_slice(codec::hex_encode(b"no such process").as_bytes());
        let script = handshake_script(vec![(&b"vAttach;7"[..], pkt(&msg)), (&b"qProcessInfo"[..], Vec::new())]);
        let server = tokio::spawn(serve(server_io, script));
        let (mut c, _) = GdbClient::handshake(client_io, HandshakeOptions::default()).await.unwrap();
        match c.attach(7).await {
            Err(GdbError::Remote { code: 1, message: Some(m) }) => assert_eq!(m, "no such process"),
            other => panic!("{other:?}"),
        }
        c.set_timeout(Duration::from_millis(100));
        assert!(matches!(c.process_info().await, Err(GdbError::Timeout(_))));
        drop(c);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn into_parts_keeps_unread_bytes() {
        let (client_io, mut server_io) = duplex(1 << 16);
        let mut c = GdbClient::raw(client_io, false);
        // Two packets arrive in one write; the client consumes only the first.
        let mut both = pkt(b"OK");
        both.extend_from_slice(&pkt(b"T05thread:1;"));
        server_io.write_all(&both).await.unwrap();
        let first = c.request(b"QSetDetachOnError:0").await.unwrap();
        assert_eq!(&first[..], b"OK");
        let parts = c.into_parts();
        assert_eq!(&parts.buffered[..], &pkt(b"T05thread:1;")[..]);
        let mut c = GdbClient::from_parts(parts);
        match c.wait_stop().await.unwrap() {
            StopEvent::Stopped(r) => assert_eq!(r.thread, Some(1)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn resume_packets() {
        assert_eq!(ResumeAction::ContinueAll.packet(), "vCont;c");
        assert_eq!(ResumeAction::ContinueWithSignal { tid: 0x1a2b, signal: 11 }.packet(), "vCont;C0b:1a2b");
        assert_eq!(ResumeAction::Step { tid: 3 }.packet(), "vCont;s:3");
        assert_eq!(ResumeAction::Raw("c".into()).packet(), "c");
    }
}
