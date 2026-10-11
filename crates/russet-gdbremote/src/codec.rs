// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Framing of the gdb-remote serial protocol: `$payload#xx` packets, `+`/`-` acknowledgements, the raw `0x03`
//! interrupt, `}` escapes and `*` run-length encoding (RD §6.4, §17.1; <https://lldb.llvm.org/resources/lldbgdbremote.html>).
//!
//! [`GdbCodec`] decodes a byte stream into [`Frame`]s and leaves packet payloads exactly as received (still
//! escaped and maybe run-length encoded), so a relay can forward them byte for byte. [`decode_payload`] turns a
//! payload into its plain bytes when somebody has to look inside.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::GdbError;

/// The largest packet the codec accepts by default (32 MiB). debugserver advertises `PacketSize=20000` (128 KiB)
/// for requests, but some replies (`jGetLoadedDynamicLibrariesInfos` with every image) are larger.
pub const DEFAULT_MAX_PACKET: usize = 32 * 1024 * 1024;

/// The interrupt byte (`^C`), sent raw outside any packet.
pub const INTERRUPT: u8 = 0x03;

/// One unit on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// `+`.
    Ack,
    /// `-`: the peer asks for a retransmission (ack mode only).
    Nack,
    /// The raw `0x03` interrupt.
    Interrupt,
    /// A `$…#xx` packet; the payload is still escaped exactly as received.
    Packet(Bytes),
    /// A `$…#xx` packet whose checksum did not match, seen in ack mode (the receiver answers `-`).
    Corrupt(Bytes),
    /// A `%…#xx` notification packet (non-stop mode; debugserver never sends one in all-stop mode).
    Notification(Bytes),
}

/// The gdb-remote codec. `ack_mode` only decides whether checksums are verified: in no-ack mode a peer may send
/// any checksum (LLDB ignores them too), so every packet is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GdbCodec {
    pub ack_mode: bool,
    pub max_packet: usize,
}

impl Default for GdbCodec {
    fn default() -> Self {
        Self { ack_mode: true, max_packet: DEFAULT_MAX_PACKET }
    }
}

impl GdbCodec {
    /// A codec in ack mode (the state of every fresh connection).
    pub fn new() -> Self {
        Self::default()
    }
}

impl Decoder for GdbCodec {
    type Item = Frame;
    type Error = GdbError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, GdbError> {
        loop {
            let Some(&first) = src.first() else {
                return Ok(None);
            };
            match first {
                b'+' => {
                    src.advance(1);
                    return Ok(Some(Frame::Ack));
                }
                b'-' => {
                    src.advance(1);
                    return Ok(Some(Frame::Nack));
                }
                INTERRUPT => {
                    src.advance(1);
                    return Ok(Some(Frame::Interrupt));
                }
                b'$' | b'%' => {
                    // '#' never appears raw inside a payload (it is escaped), so the first one ends the packet.
                    let Some(hash) = src[1..].iter().position(|&b| b == b'#').map(|i| i + 1) else {
                        if src.len() > self.max_packet.saturating_add(4) {
                            return Err(GdbError::Protocol(format!(
                                "packet larger than {} bytes without a terminator",
                                self.max_packet
                            )));
                        }
                        return Ok(None);
                    };
                    if hash - 1 > self.max_packet {
                        return Err(GdbError::Protocol(format!(
                            "packet of {} bytes exceeds the {}-byte limit",
                            hash - 1,
                            self.max_packet
                        )));
                    }
                    if src.len() < hash + 3 {
                        return Ok(None);
                    }
                    let whole = src.split_to(hash + 3).freeze();
                    let payload = whole.slice(1..hash);
                    let sent = parse_hex_byte(whole[hash + 1], whole[hash + 2]);
                    if first == b'%' {
                        return Ok(Some(Frame::Notification(payload)));
                    }
                    if !self.ack_mode || sent == Some(checksum(&payload)) {
                        return Ok(Some(Frame::Packet(payload)));
                    }
                    return Ok(Some(Frame::Corrupt(payload)));
                }
                // Noise between frames (a stray newline from a terminal, a byte of a broken frame): skip it, as
                // LLDB does.
                _ => src.advance(1),
            }
        }
    }
}

impl Encoder<&[u8]> for GdbCodec {
    type Error = GdbError;

    /// Frames an already escaped payload as `$payload#xx`.
    fn encode(&mut self, payload: &[u8], dst: &mut BytesMut) -> Result<(), GdbError> {
        if payload.len() > self.max_packet {
            return Err(GdbError::Protocol(format!(
                "packet of {} bytes exceeds the {}-byte limit",
                payload.len(),
                self.max_packet
            )));
        }
        put_frame(dst, b'$', payload);
        Ok(())
    }
}

impl Encoder<Frame> for GdbCodec {
    type Error = GdbError;

    fn encode(&mut self, frame: Frame, dst: &mut BytesMut) -> Result<(), GdbError> {
        match frame {
            Frame::Ack => dst.put_u8(b'+'),
            Frame::Nack => dst.put_u8(b'-'),
            Frame::Interrupt => dst.put_u8(INTERRUPT),
            Frame::Packet(p) | Frame::Corrupt(p) => self.encode(&p[..], dst)?,
            Frame::Notification(p) => put_frame(dst, b'%', &p),
        }
        Ok(())
    }
}

fn put_frame(dst: &mut BytesMut, lead: u8, payload: &[u8]) {
    dst.reserve(payload.len() + 4);
    dst.put_u8(lead);
    dst.put_slice(payload);
    dst.put_u8(b'#');
    let [hi, lo] = hex_pair(checksum(payload));
    dst.put_u8(hi);
    dst.put_u8(lo);
}

/// The modulo-256 sum of the payload bytes.
pub fn checksum(payload: &[u8]) -> u8 {
    payload.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

/// `$payload#xx` for an already escaped payload.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = BytesMut::with_capacity(payload.len() + 4);
    put_frame(&mut out, b'$', payload);
    out.to_vec()
}

/// Escapes binary data for a packet: `#`, `$`, `}` and `*` become `}` followed by the byte XOR 0x20.
pub fn escape_binary(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 8);
    for &b in data {
        if matches!(b, b'#' | b'$' | b'}' | b'*') {
            out.push(b'}');
            out.push(b ^ 0x20);
        } else {
            out.push(b);
        }
    }
    out
}

/// Decodes a received payload: `}` escapes and `*` run-length encoding (`c*n` repeats `c` another `n - 29`
/// times). Applying it to a plain text payload is harmless, because those bytes never appear raw in one.
pub fn decode_payload(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut i = 0;
    while i < payload.len() {
        match payload[i] {
            b'}' if i + 1 < payload.len() => {
                out.push(payload[i + 1] ^ 0x20);
                i += 2;
            }
            b'*' if i + 1 < payload.len() && !out.is_empty() => {
                let repeat = usize::from(payload[i + 1].saturating_sub(29));
                let last = out[out.len() - 1];
                out.extend(std::iter::repeat_n(last, repeat));
                i += 2;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// The RD §17.1 name of [`decode_payload`].
pub fn unescape(payload: &[u8]) -> Vec<u8> {
    decode_payload(payload)
}

/// Lower-case hex of every byte.
pub fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for &b in data {
        let [hi, lo] = hex_pair(b);
        s.push(char::from(hi));
        s.push(char::from(lo));
    }
    s
}

/// Decodes pairs of hex digits; `None` on an odd length or a non-hex digit.
pub fn hex_decode(text: &[u8]) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_chunks::<2>().0.iter().map(|&[hi, lo]| parse_hex_byte(hi, lo)).collect()
}

/// A textual hex number, most significant digit first (addresses, lengths, pids, thread ids). At most 16 digits.
pub fn parse_hex_u64(text: &[u8]) -> Option<u64> {
    if text.is_empty() || text.len() > 16 {
        return None;
    }
    let mut v = 0u64;
    for &c in text {
        v = (v << 4) | u64::from(hex_digit(c)?);
    }
    Some(v)
}

/// A register value as debugserver sends it: target-order (little-endian) bytes in hex. Values wider than eight
/// bytes give `None`.
pub fn parse_le_hex_u64(text: &[u8]) -> Option<u64> {
    let bytes = hex_decode(text)?;
    if bytes.is_empty() || bytes.len() > 8 {
        return None;
    }
    Some(bytes.iter().rev().fold(0u64, |acc, &b| (acc << 8) | u64::from(b)))
}

/// `value` as `size` little-endian bytes in hex (the `P` packet's value format).
pub fn le_hex(value: u64, size: usize) -> String {
    let bytes = value.to_le_bytes();
    hex_encode(&bytes[..size.min(8)])
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn parse_hex_byte(hi: u8, lo: u8) -> Option<u8> {
    Some((hex_digit(hi)? << 4) | hex_digit(lo)?)
}

fn hex_pair(b: u8) -> [u8; 2] {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    [DIGITS[usize::from(b >> 4)], DIGITS[usize::from(b & 0xf)]]
}

/// Splits `key:value;key:value;…` (the format of stop replies, `qHostInfo`, `qProcessInfo`,
/// `qMemoryRegionInfo` and `qRegisterInfo`). Keys may repeat (`medata`), so the pairs stay in order. An entry
/// without a colon becomes a key with an empty value.
pub fn parse_kv(text: &[u8]) -> Vec<(String, String)> {
    text.split(|&b| b == b';')
        .filter(|e| !e.is_empty())
        .map(|e| match e.iter().position(|&b| b == b':') {
            Some(i) => {
                (String::from_utf8_lossy(&e[..i]).into_owned(), String::from_utf8_lossy(&e[i + 1..]).into_owned())
            }
            None => (String::from_utf8_lossy(e).into_owned(), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn decode_all(codec: &mut GdbCodec, bytes: &[u8]) -> Vec<Frame> {
        let mut buf = BytesMut::from(bytes);
        let mut out = Vec::new();
        while let Some(f) = codec.decode(&mut buf).unwrap() {
            out.push(f);
        }
        assert!(buf.is_empty(), "left over: {buf:?}");
        out
    }

    #[test]
    fn checksums_match_known_packets() {
        assert_eq!(checksum(b"OK"), 0x9a);
        assert_eq!(frame(b"OK"), b"$OK#9a");
        assert_eq!(frame(b"QStartNoAckMode"), b"$QStartNoAckMode#b0");
        assert_eq!(frame(b""), b"$#00");
        assert_eq!(frame(b"vCont;c"), b"$vCont;c#a8");
    }

    #[test]
    fn acks_interrupts_and_packets_decode_in_order() {
        let mut codec = GdbCodec::new();
        let frames = decode_all(&mut codec, b"+$OK#9a-\x03$#00");
        assert_eq!(
            frames,
            vec![
                Frame::Ack,
                Frame::Packet(Bytes::from_static(b"OK")),
                Frame::Nack,
                Frame::Interrupt,
                Frame::Packet(Bytes::new())
            ]
        );
    }

    #[test]
    fn partial_frames_wait_for_more_bytes() {
        let mut codec = GdbCodec::new();
        let mut buf = BytesMut::from(&b"$qSupp"[..]);
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        buf.extend_from_slice(b"orted#");
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        buf.extend_from_slice(b"3");
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        buf.extend_from_slice(b"7+");
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(Frame::Packet(Bytes::from_static(b"qSupported"))));
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(Frame::Ack));
    }

    #[test]
    fn bad_checksums_are_corrupt_in_ack_mode_only() {
        let mut ack = GdbCodec::new();
        assert_eq!(decode_all(&mut ack, b"$OK#00"), vec![Frame::Corrupt(Bytes::from_static(b"OK"))]);
        let mut no_ack = GdbCodec { ack_mode: false, ..GdbCodec::new() };
        assert_eq!(decode_all(&mut no_ack, b"$OK#00"), vec![Frame::Packet(Bytes::from_static(b"OK"))]);
        // Upper-case checksum digits are fine too.
        assert_eq!(decode_all(&mut ack, b"$vCont;c#A8"), vec![Frame::Packet(Bytes::from_static(b"vCont;c"))]);
    }

    #[test]
    fn noise_between_frames_is_skipped_and_notifications_are_kept() {
        let mut codec = GdbCodec::new();
        let frames = decode_all(&mut codec, b"\r\n$OK#9a%Stop:T05#00");
        assert_eq!(
            frames,
            vec![Frame::Packet(Bytes::from_static(b"OK")), Frame::Notification(Bytes::from_static(b"Stop:T05"))]
        );
    }

    #[test]
    fn oversized_packets_are_rejected() {
        let mut codec = GdbCodec { ack_mode: true, max_packet: 8 };
        let mut buf = BytesMut::from(&b"$0123456789abcdef"[..]);
        assert!(matches!(codec.decode(&mut buf), Err(GdbError::Protocol(_))));
        let mut buf = BytesMut::from(&b"$0123456789#00"[..]);
        assert!(matches!(codec.decode(&mut buf), Err(GdbError::Protocol(_))));
    }

    #[test]
    fn encoders_frame_payloads_and_control_bytes() {
        let mut codec = GdbCodec::new();
        let mut out = BytesMut::new();
        codec.encode(&b"m1000,4"[..], &mut out).unwrap();
        codec.encode(Frame::Ack, &mut out).unwrap();
        codec.encode(Frame::Interrupt, &mut out).unwrap();
        codec.encode(Frame::Notification(Bytes::from_static(b"Stop:T05")), &mut out).unwrap();
        let mut expected = frame(b"m1000,4");
        expected.extend_from_slice(b"+\x03%Stop:T05#");
        expected.extend_from_slice(hex_encode(&[checksum(b"Stop:T05")]).as_bytes());
        assert_eq!(&out[..], &expected[..]);
    }

    #[test]
    fn escapes_round_trip_every_byte() {
        let data: Vec<u8> = (0u8..=255).collect();
        let escaped = escape_binary(&data);
        assert!(!escaped.iter().any(|&b| b == b'#' || b == b'$' || b == b'*'));
        assert_eq!(decode_payload(&escaped), data);
        // A framed binary packet survives the codec unchanged.
        let mut codec = GdbCodec::new();
        let frames = decode_all(&mut codec, &frame(&escaped));
        assert_eq!(frames, vec![Frame::Packet(Bytes::from(escaped))]);
    }

    #[test]
    fn run_length_encoding_expands() {
        // "0* " is '0' plus three more copies (' ' is 32 = 3 + 29).
        assert_eq!(decode_payload(b"0* "), b"0000");
        // '"' is 34: five more copies of 'b'.
        assert_eq!(decode_payload(b"ab*\"c"), b"abbbbbbc");
        // An escaped byte can be repeated.
        assert_eq!(decode_payload(b"}]* "), b"}}}}");
        // A '*' with nothing before it stays literal.
        assert_eq!(decode_payload(b"*x"), b"*x");
    }

    #[test]
    fn hex_helpers() {
        assert_eq!(hex_encode(b"IHELPED!"), "4948454c50454421");
        assert_eq!(hex_decode(b"4948454c50454421").unwrap(), b"IHELPED!");
        assert_eq!(hex_decode(b"abc"), None);
        assert_eq!(hex_decode(b"zz"), None);
        assert_eq!(parse_hex_u64(b"100000000"), Some(0x1_0000_0000));
        assert_eq!(parse_hex_u64(b"FFFFFFFFFFFFFFFF"), Some(u64::MAX));
        assert_eq!(parse_hex_u64(b"1FFFFFFFFFFFFFFFF"), None);
        assert_eq!(parse_hex_u64(b""), None);
        assert_eq!(parse_le_hex_u64(b"0010000001000000"), Some(0x1_0000_1000));
        assert_eq!(parse_le_hex_u64(b"00000000000000000000000000000000"), None);
        assert_eq!(le_hex(0x1_0000_1000, 8), "0010000001000000");
        assert_eq!(le_hex(0x8000_0000, 4), "00000080");
    }

    #[test]
    fn key_value_lists_keep_order_and_repeats() {
        let kv = parse_kv(b"metype:6;mecount:2;medata:1;medata:0;flag;");
        assert_eq!(
            kv,
            vec![
                ("metype".into(), "6".into()),
                ("mecount".into(), "2".into()),
                ("medata".into(), "1".into()),
                ("medata".into(), "0".into()),
                ("flag".into(), String::new()),
            ]
        );
    }
}
