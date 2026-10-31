// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A frame-level relay between a debugger (downstream, LLDB) and a stub (upstream, debugserver) with separate
//! ack state per side and hooks that may answer, drop or rewrite packets (RD §6.4–§6.5).
//!
//! Packets are forwarded byte for byte (still escaped) unless a hook rewrites them. The relay keeps track of the
//! debugger's outstanding requests so that it knows which upstream packet answers which request; a stop or exit
//! reply that arrives while a resume is outstanding answers that resume. The debugger's `QStartNoAckMode` is
//! answered locally, because the upstream side is already in no-ack mode after the broker's handshake.

use std::collections::VecDeque;
use std::future::Future;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::codec::Decoder;

use crate::GdbError;
use crate::client::RawSession;
use crate::codec::{self, Frame, GdbCodec, INTERRUPT};
use crate::stop::{self, StopEvent};

/// What to do with a packet from the debugger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownstreamAction {
    /// Send it upstream unchanged.
    Forward,
    /// Answer it locally with this (plain, unescaped) payload; nothing goes upstream.
    Reply(Vec<u8>),
    /// Ignore it.
    Drop,
}

/// What to do with a stop or exit reply that answers the debugger's resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamStopAction {
    /// Hand it to the debugger unchanged.
    Forward,
    /// Hand the debugger this (plain) stop reply instead, for example with a thread scrubbed from `jstopinfo`.
    Replace(Vec<u8>),
    /// Keep it from the debugger: send `requests` upstream (their replies go to
    /// [`InterposeHooks::on_injected_reply`]), then `resume`, whose stop answers the debugger's resume again
    /// (JIT servicing in interposer mode, RD §6.5).
    Swallow { requests: Vec<Vec<u8>>, resume: Vec<u8> },
}

/// Hooks of the relay. Every packet is passed decoded (escapes and run-length encoding removed).
pub trait InterposeHooks: Send {
    /// A packet from the debugger.
    fn on_downstream(&mut self, _pkt: &[u8]) -> DownstreamAction {
        DownstreamAction::Forward
    }
    /// A stop or exit that answers the debugger's resume.
    fn on_upstream_stop(&mut self, _stop: &StopEvent) -> UpstreamStopAction {
        UpstreamStopAction::Forward
    }
    /// A reply to a debugger request, before it is forwarded (for example dropping `SupportedCompressions`
    /// from `qSupported`, or scrubbing `jThreadsInfo`).
    fn filter_upstream_reply(&mut self, _request: &[u8], _reply: &mut Vec<u8>) {}
    /// The reply to a packet the hooks injected through [`UpstreamStopAction::Swallow`].
    fn on_injected_reply(&mut self, _request: &[u8], _reply: &[u8]) {}
    /// The debugger sent the raw interrupt; return `false` to keep it from the stub.
    fn on_interrupt(&mut self) -> bool {
        true
    }
}

/// Hooks that change nothing: a plain relay.
#[derive(Debug, Clone, Copy, Default)]
pub struct PassThrough;

impl InterposeHooks for PassThrough {}

/// Why [`Interposer::run`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayEnd {
    /// The debugger closed its connection.
    DownstreamClosed,
    /// The stub closed its connection.
    UpstreamClosed,
    /// The caller's stop future fired.
    Stopped,
}

/// What [`Interposer::run`] hands back.
#[derive(Debug)]
pub struct RelayOutcome<U, D> {
    pub end: RelayEnd,
    /// The stub side, with any bytes read but not relayed; the broker applies its detach/kill policy on it.
    pub upstream: RawSession<U>,
    pub downstream: D,
    /// Whether a resume was outstanding (the inferior may be running).
    pub resume_pending: bool,
    pub stats: RelayStats,
}

/// Packet counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayStats {
    pub downstream_packets: u64,
    pub upstream_packets: u64,
    pub answered_locally: u64,
    pub swallowed_stops: u64,
}

enum Query {
    Debugger(Bytes),
    Injected(Bytes),
}

/// The relay. `upstream` is a [`RawSession`] from [`crate::GdbClient::into_parts`]; `downstream` is the
/// debugger's fresh connection (in ack mode).
#[derive(Debug)]
pub struct Interposer<U, D> {
    upstream: RawSession<U>,
    downstream: D,
}

impl<U, D> Interposer<U, D>
where
    U: AsyncRead + AsyncWrite + Unpin + Send,
    D: AsyncRead + AsyncWrite + Unpin + Send,
{
    pub fn new(upstream: RawSession<U>, downstream: D) -> Self {
        Self { upstream, downstream }
    }

    /// Relays until a side closes or `stop` resolves.
    pub async fn run<H: InterposeHooks, F: Future<Output = ()>>(
        self,
        hooks: &mut H,
        stop: F,
    ) -> Result<RelayOutcome<U, D>, GdbError> {
        let Interposer { upstream, mut downstream } = self;
        let RawSession { stream: mut up, buffered: mut up_buf, ack_mode: up_ack, server } = upstream;
        let mut up_codec = GdbCodec { ack_mode: up_ack, ..GdbCodec::new() };
        let mut down_codec = GdbCodec::new();
        let mut down_buf = BytesMut::with_capacity(64 * 1024);
        let mut queries: VecDeque<Query> = VecDeque::new();
        let mut resume: Option<Bytes> = None;
        let mut last_down: Option<Vec<u8>> = None;
        let mut last_up: Option<Vec<u8>> = None;
        let mut stats = RelayStats::default();
        tokio::pin!(stop);

        let end = 'relay: loop {
            // Drain complete frames first, so bytes buffered before the relay started are handled.
            while let Some(frame) = up_codec.decode(&mut up_buf)? {
                match frame {
                    Frame::Packet(raw) => {
                        if up_codec.ack_mode {
                            up.write_all(b"+").await?;
                        }
                        stats.upstream_packets += 1;
                        let decoded = codec::decode_payload(&raw);
                        if stop::is_output_packet(&decoded) {
                            if !send_down(&mut downstream, &raw, &mut last_down).await {
                                break 'relay RelayEnd::DownstreamClosed;
                            }
                            continue;
                        }
                        if resume.is_some() && stop::is_stop_packet(&decoded) {
                            let event = stop::parse_stop(&decoded)?;
                            match hooks.on_upstream_stop(&event) {
                                UpstreamStopAction::Forward => {
                                    resume = None;
                                    if !send_down(&mut downstream, &raw, &mut last_down).await {
                                        break 'relay RelayEnd::DownstreamClosed;
                                    }
                                }
                                UpstreamStopAction::Replace(plain) => {
                                    resume = None;
                                    let escaped = codec::escape_binary(&plain);
                                    if !send_down(&mut downstream, &escaped, &mut last_down).await {
                                        break 'relay RelayEnd::DownstreamClosed;
                                    }
                                }
                                UpstreamStopAction::Swallow { requests, resume: again } => {
                                    stats.swallowed_stops += 1;
                                    let mut batch = Vec::new();
                                    for r in requests {
                                        batch.extend_from_slice(&codec::frame(&r));
                                        queries.push_back(Query::Injected(Bytes::from(r)));
                                    }
                                    batch.extend_from_slice(&codec::frame(&again));
                                    send_up(&mut up, &batch, &mut last_up).await?;
                                }
                            }
                            continue;
                        }
                        match queries.pop_front() {
                            Some(Query::Debugger(request)) => {
                                let mut reply = decoded.clone();
                                hooks.filter_upstream_reply(&request, &mut reply);
                                let out = if reply == decoded { raw.to_vec() } else { codec::escape_binary(&reply) };
                                if !send_down(&mut downstream, &out, &mut last_down).await {
                                    break 'relay RelayEnd::DownstreamClosed;
                                }
                            }
                            Some(Query::Injected(request)) => hooks.on_injected_reply(&request, &decoded),
                            // Unsolicited (an async stop the debugger did not ask for): the debugger decides.
                            None => {
                                if !send_down(&mut downstream, &raw, &mut last_down).await {
                                    break 'relay RelayEnd::DownstreamClosed;
                                }
                            }
                        }
                    }
                    Frame::Nack => {
                        if let Some(again) = last_up.clone() {
                            up.write_all(&again).await?;
                        }
                    }
                    Frame::Corrupt(_) => up.write_all(b"-").await?,
                    Frame::Ack | Frame::Interrupt | Frame::Notification(_) => {}
                }
            }
            while let Some(frame) = down_codec.decode(&mut down_buf)? {
                match frame {
                    Frame::Packet(raw) => {
                        if down_codec.ack_mode && !write_down(&mut downstream, b"+").await {
                            break 'relay RelayEnd::DownstreamClosed;
                        }
                        stats.downstream_packets += 1;
                        let decoded = codec::decode_payload(&raw);
                        if decoded == b"QStartNoAckMode" {
                            stats.answered_locally += 1;
                            if !send_down(&mut downstream, b"OK", &mut last_down).await {
                                break 'relay RelayEnd::DownstreamClosed;
                            }
                            down_codec.ack_mode = false;
                            continue;
                        }
                        match hooks.on_downstream(&decoded) {
                            DownstreamAction::Forward => {
                                let decoded = Bytes::from(decoded);
                                if is_resume(&decoded) {
                                    resume = Some(decoded);
                                } else {
                                    queries.push_back(Query::Debugger(decoded));
                                }
                                send_up(&mut up, &codec::frame(&raw), &mut last_up).await?;
                            }
                            DownstreamAction::Reply(plain) => {
                                stats.answered_locally += 1;
                                let escaped = codec::escape_binary(&plain);
                                if !send_down(&mut downstream, &escaped, &mut last_down).await {
                                    break 'relay RelayEnd::DownstreamClosed;
                                }
                            }
                            DownstreamAction::Drop => {}
                        }
                    }
                    Frame::Interrupt => {
                        if hooks.on_interrupt() {
                            up.write_all(&[INTERRUPT]).await?;
                            up.flush().await?;
                        }
                    }
                    Frame::Nack => {
                        if let Some(again) = last_down.clone()
                            && !write_down(&mut downstream, &again).await
                        {
                            break 'relay RelayEnd::DownstreamClosed;
                        }
                    }
                    Frame::Corrupt(_) => {
                        if !write_down(&mut downstream, b"-").await {
                            break 'relay RelayEnd::DownstreamClosed;
                        }
                    }
                    Frame::Ack | Frame::Notification(_) => {}
                }
            }
            up_buf.reserve(64 * 1024);
            down_buf.reserve(64 * 1024);
            tokio::select! {
                r = up.read_buf(&mut up_buf) => {
                    match r {
                        Ok(0) | Err(_) => break 'relay RelayEnd::UpstreamClosed,
                        Ok(_) => {}
                    }
                }
                r = downstream.read_buf(&mut down_buf) => {
                    match r {
                        Ok(0) | Err(_) => break 'relay RelayEnd::DownstreamClosed,
                        Ok(_) => {}
                    }
                }
                () = &mut stop => break 'relay RelayEnd::Stopped,
            }
        };
        Ok(RelayOutcome {
            end,
            upstream: RawSession { stream: up, buffered: up_buf, ack_mode: up_codec.ack_mode, server },
            downstream,
            resume_pending: resume.is_some(),
            stats,
        })
    }
}

/// Whether a (decoded) debugger packet resumes the inferior: `c`, `C`, `s`, `S`, `vCont;…` (not `vCont?`).
pub fn is_resume(pkt: &[u8]) -> bool {
    match pkt.first() {
        Some(b'c' | b'C' | b's' | b'S') => {
            // `c`, `cADDR`, `C05`, `s`, `S05` — but not the queries that start with these letters.
            pkt.len() == 1 || pkt[1..].iter().all(|b| b.is_ascii_hexdigit() || *b == b';')
        }
        _ => pkt.starts_with(b"vCont;"),
    }
}

async fn send_up<U: AsyncWrite + Unpin>(up: &mut U, framed: &[u8], last: &mut Option<Vec<u8>>) -> Result<(), GdbError> {
    up.write_all(framed).await?;
    up.flush().await?;
    *last = Some(framed.to_vec());
    Ok(())
}

/// Frames an escaped payload for the debugger; `false` when the debugger is gone.
async fn send_down<D: AsyncWrite + Unpin>(down: &mut D, escaped: &[u8], last: &mut Option<Vec<u8>>) -> bool {
    let framed = codec::frame(escaped);
    let ok = write_down(down, &framed).await;
    *last = Some(framed);
    ok
}

async fn write_down<D: AsyncWrite + Unpin>(down: &mut D, bytes: &[u8]) -> bool {
    down.write_all(bytes).await.is_ok() && down.flush().await.is_ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::time::Duration;

    use tokio::io::{DuplexStream, duplex};

    use super::*;
    use crate::client::{GdbClient, ServerInfo};

    /// A stub that answers by rules; it starts in no-ack mode, as after the broker's handshake.
    async fn stub(mut io: DuplexStream) -> Vec<Vec<u8>> {
        let mut seen = Vec::new();
        let mut codec = GdbCodec { ack_mode: false, ..GdbCodec::new() };
        let mut buf = BytesMut::new();
        loop {
            let frame = loop {
                if let Some(f) = codec.decode(&mut buf).unwrap() {
                    break Some(f);
                }
                if io.read_buf(&mut buf).await.unwrap() == 0 {
                    break None;
                }
            };
            let Some(frame) = frame else { return seen };
            let p = match frame {
                Frame::Packet(p) => codec::decode_payload(&p),
                Frame::Interrupt => {
                    io.write_all(&codec::frame(b"T11thread:1;reason:signal;")).await.unwrap();
                    continue;
                }
                _ => continue,
            };
            seen.push(p.clone());
            let reply: Vec<u8> = match &p[..] {
                p if p.starts_with(b"qSupported") => {
                    b"PacketSize=20000;SupportedCompressions=lzfse,zlib-deflate;qEcho+".to_vec()
                }
                b"vCont;c" => {
                    // Output first, then the stop.
                    io.write_all(&codec::frame(b"O6869")).await.unwrap();
                    b"T05thread:1;reason:breakpoint;".to_vec()
                }
                b"vCont;C0b:1" => continue, // the stop comes only after an interrupt
                b"k" => b"X09".to_vec(),
                b"D" => b"OK".to_vec(),
                p if p.starts_with(b"M") || p.starts_with(b"P") => b"OK".to_vec(),
                _ => b"OK".to_vec(),
            };
            io.write_all(&codec::frame(&reply)).await.unwrap();
        }
    }

    fn upstream(io: DuplexStream) -> RawSession<DuplexStream> {
        RawSession { stream: io, buffered: BytesMut::new(), ack_mode: false, server: ServerInfo::default() }
    }

    /// Hooks like the broker's: refuse attaches, strip compressions, count stops.
    #[derive(Default)]
    struct Broker {
        stops: usize,
        swallow_next: bool,
        injected: Vec<Vec<u8>>,
    }

    impl InterposeHooks for Broker {
        fn on_downstream(&mut self, pkt: &[u8]) -> DownstreamAction {
            if pkt.starts_with(b"vAttach") {
                return DownstreamAction::Reply(b"E16".to_vec());
            }
            DownstreamAction::Forward
        }
        fn on_upstream_stop(&mut self, _stop: &StopEvent) -> UpstreamStopAction {
            self.stops += 1;
            if std::mem::take(&mut self.swallow_next) {
                return UpstreamStopAction::Swallow {
                    requests: vec![b"M1000,1:00".to_vec(), b"P20=0010000000000000;thread:1;".to_vec()],
                    resume: b"vCont;c".to_vec(),
                };
            }
            UpstreamStopAction::Forward
        }
        fn filter_upstream_reply(&mut self, request: &[u8], reply: &mut Vec<u8>) {
            if request.starts_with(b"qSupported") {
                let text = String::from_utf8_lossy(reply).into_owned();
                *reply = text
                    .split(';')
                    .filter(|f| !f.starts_with("SupportedCompressions="))
                    .collect::<Vec<_>>()
                    .join(";")
                    .into_bytes();
            }
        }
        fn on_injected_reply(&mut self, request: &[u8], reply: &[u8]) {
            assert_eq!(reply, b"OK");
            self.injected.push(request.to_vec());
        }
    }

    #[tokio::test]
    async fn an_lldb_like_client_talks_through_the_relay() {
        let (up_relay, up_stub) = duplex(1 << 16);
        let (down_relay, down_lldb) = duplex(1 << 16);
        let stub_task = tokio::spawn(stub(up_stub));
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let relay = tokio::spawn(async move {
            let mut hooks = Broker { swallow_next: true, ..Broker::default() };
            let out = Interposer::new(upstream(up_relay), down_relay)
                .run(&mut hooks, async {
                    let _ = stop_rx.await;
                })
                .await
                .unwrap();
            (out.end, hooks.stops, hooks.injected, out.stats, out.resume_pending)
        });

        // The "debugger": a GdbClient in ack mode, like LLDB.
        let mut lldb =
            GdbClient::handshake(down_lldb, crate::HandshakeOptions { host_info: false, ..Default::default() })
                .await
                .unwrap()
                .0;
        assert!(lldb.server().no_ack, "QStartNoAckMode is answered locally");
        assert!(lldb.server().compressions.is_empty(), "compressions are stripped");
        assert_eq!(lldb.server().features.get("qEcho").map(String::as_str), Some("+"));
        // Attaching is refused by the hooks.
        assert!(matches!(lldb.attach(42).await, Err(GdbError::Remote { code: 0x16, .. })));
        // The first stop is swallowed and serviced; the second one arrives, after the output.
        lldb.resume(crate::ResumeAction::ContinueAll).await.unwrap();
        assert_eq!(lldb.wait_stop().await.unwrap(), StopEvent::Output(b"hi".to_vec()));
        assert_eq!(lldb.wait_stop().await.unwrap(), StopEvent::Output(b"hi".to_vec()));
        assert!(matches!(lldb.wait_stop().await.unwrap(), StopEvent::Stopped(_)));
        // An interrupt goes through as the raw byte.
        lldb.resume(crate::ResumeAction::ContinueWithSignal { tid: 1, signal: 11 }).await.unwrap();
        let stop = lldb.interrupt().await.unwrap();
        assert_eq!(stop.signal, 0x11);
        lldb.request_ok(b"QSetDetachOnError:0").await.unwrap();
        stop_tx.send(()).unwrap();
        let (end, stops, injected, stats, resume_pending) = relay.await.unwrap();
        assert_eq!(end, RelayEnd::Stopped);
        assert_eq!(stops, 3);
        assert_eq!(injected, vec![b"M1000,1:00".to_vec(), b"P20=0010000000000000;thread:1;".to_vec()]);
        assert_eq!(stats.swallowed_stops, 1);
        assert!(!resume_pending);
        drop(lldb);
        let seen = stub_task.await.unwrap();
        assert!(!seen.iter().any(|p| p.starts_with(b"vAttach")), "refused packets never reach the stub");
        assert!(!seen.iter().any(|p| p == b"QStartNoAckMode"), "no-ack is local");
    }

    #[tokio::test]
    async fn a_closed_debugger_hands_the_stub_back() {
        let (up_relay, up_stub) = duplex(1 << 16);
        let (down_relay, down_lldb) = duplex(1 << 16);
        let stub_task = tokio::spawn(stub(up_stub));
        let relay = tokio::spawn(async move {
            Interposer::new(upstream(up_relay), down_relay)
                .run(&mut PassThrough, std::future::pending::<()>())
                .await
                .unwrap()
        });
        let mut lldb = GdbClient::raw(down_lldb, true);
        lldb.write_raw(b"+").await.unwrap();
        lldb.request_ok(b"QSetDetachOnError:0").await.unwrap();
        drop(lldb);
        let out = tokio::time::timeout(Duration::from_secs(5), relay).await.unwrap().unwrap();
        assert_eq!(out.end, RelayEnd::DownstreamClosed);
        // The broker can still kill through the returned upstream.
        let client = GdbClient::from_parts(out.upstream);
        assert_eq!(client.kill().await.unwrap(), Some(StopEvent::Terminated { signal: 9 }));
        let seen = stub_task.await.unwrap();
        assert_eq!(seen.last().map(Vec::as_slice), Some(&b"k"[..]));
    }

    #[test]
    fn resume_detection() {
        for p in [&b"c"[..], b"C05", b"s", b"S0b", b"vCont;c", b"vCont;s:1a2b", b"c1000"] {
            assert!(is_resume(p), "{}", String::from_utf8_lossy(p));
        }
        for p in [&b"vCont?"[..], b"qSupported", b"qC", b"QSetDetachOnError:0", b"vAttach;1", b"m1000,4"] {
            assert!(!is_resume(p), "{}", String::from_utf8_lossy(p));
        }
    }
}
