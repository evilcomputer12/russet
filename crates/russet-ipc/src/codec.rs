// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! LSP base-protocol framing (ARCHITECTURE §7.2): `Content-Length: <n>\r\n\r\n<n bytes of UTF-8 JSON>`.
//!
//! Header names are case-insensitive; `Content-Type` and unknown headers are ignored. A frame larger than
//! [`MAX_FRAME_BYTES`](crate::MAX_FRAME_BYTES) is [`CodecError::FrameTooLarge`]: the connection answers
//! `ipc.frameTooLarge` and closes. Headers are limited to 8 KiB, so a peer cannot make the reader buffer
//! without bound before a body even starts.

use std::fmt;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

/// The longest header section accepted.
const MAX_HEADER_BYTES: usize = 8 * 1024;

/// Content-Length framing for JSON-RPC bodies.
#[derive(Debug, Clone)]
pub struct LspCodec {
    max_frame: usize,
    /// The body length once the headers of the current frame were read.
    pending_body: Option<usize>,
}

impl LspCodec {
    /// A codec that accepts frames up to `max_frame` bytes.
    pub fn new(max_frame: usize) -> Self {
        Self { max_frame, pending_body: None }
    }
}

impl Default for LspCodec {
    fn default() -> Self {
        Self::new(crate::MAX_FRAME_BYTES)
    }
}

/// Why a stream of frames cannot continue.
#[derive(Debug)]
pub enum CodecError {
    Io(std::io::Error),
    /// The announced (or outgoing) body is larger than the limit.
    FrameTooLarge(usize),
    /// The header section is malformed: no `Content-Length`, a bad value, or longer than 8 KiB.
    BadHeader(String),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::FrameTooLarge(n) => write!(f, "a frame of {n} bytes is larger than the limit"),
            Self::BadHeader(why) => write!(f, "bad frame header: {why}"),
        }
    }
}

impl std::error::Error for CodecError {}

impl From<std::io::Error> for CodecError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(headers: &[u8]) -> Result<usize, CodecError> {
    let text = std::str::from_utf8(headers).map_err(|_| CodecError::BadHeader("headers are not UTF-8".into()))?;
    let mut length = None;
    for line in text.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            return Err(CodecError::BadHeader(format!("`{line}` is not a header")));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let n = value
                .trim()
                .parse::<usize>()
                .map_err(|_| CodecError::BadHeader(format!("bad Content-Length `{}`", value.trim())))?;
            length = Some(n);
        }
    }
    length.ok_or_else(|| CodecError::BadHeader("no Content-Length".into()))
}

impl Decoder for LspCodec {
    type Item = Bytes;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Bytes>, CodecError> {
        if self.pending_body.is_none() {
            let Some(end) = find_header_end(src) else {
                if src.len() > MAX_HEADER_BYTES {
                    return Err(CodecError::BadHeader("the header section is longer than 8 KiB".into()));
                }
                return Ok(None);
            };
            if end > MAX_HEADER_BYTES {
                return Err(CodecError::BadHeader("the header section is longer than 8 KiB".into()));
            }
            let length = parse_content_length(&src[..end])?;
            if length > self.max_frame {
                return Err(CodecError::FrameTooLarge(length));
            }
            src.advance(end + 4);
            self.pending_body = Some(length);
        }
        let Some(length) = self.pending_body else { return Ok(None) };
        if src.len() < length {
            src.reserve(length - src.len());
            return Ok(None);
        }
        self.pending_body = None;
        Ok(Some(src.split_to(length).freeze()))
    }
}

impl Encoder<Bytes> for LspCodec {
    type Error = CodecError;

    fn encode(&mut self, body: Bytes, dst: &mut BytesMut) -> Result<(), CodecError> {
        if body.len() > self.max_frame {
            return Err(CodecError::FrameTooLarge(body.len()));
        }
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        dst.reserve(header.len() + body.len());
        dst.put_slice(header.as_bytes());
        dst.put_slice(&body);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(codec: &mut LspCodec, input: &[u8]) -> Result<Vec<Bytes>, CodecError> {
        let mut buf = BytesMut::from(input);
        let mut out = Vec::new();
        while let Some(frame) = codec.decode(&mut buf)? {
            out.push(frame);
        }
        Ok(out)
    }

    #[test]
    fn frames_decode_with_any_header_case_and_extra_headers() {
        let input = b"content-length: 2\r\n\r\n{}CONTENT-LENGTH: 4\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n[1,2]";
        let mut codec = LspCodec::default();
        let frames = decode_all(&mut codec, input).expect("decodes");
        assert_eq!(frames, vec![Bytes::from_static(b"{}"), Bytes::from_static(b"[1,2")]);
    }

    #[test]
    fn oversized_and_malformed_frames_are_errors() {
        let mut codec = LspCodec::new(10);
        assert!(matches!(decode_all(&mut codec, b"Content-Length: 11\r\n\r\n"), Err(CodecError::FrameTooLarge(11))));
        let mut codec = LspCodec::default();
        assert!(matches!(decode_all(&mut codec, b"Content-Type: x\r\n\r\n{}"), Err(CodecError::BadHeader(_))));
        let mut codec = LspCodec::default();
        assert!(matches!(decode_all(&mut codec, b"Content-Length: two\r\n\r\n"), Err(CodecError::BadHeader(_))));
        let mut codec = LspCodec::default();
        let endless = vec![b'a'; MAX_HEADER_BYTES + 1];
        assert!(matches!(decode_all(&mut codec, &endless), Err(CodecError::BadHeader(_))));
        let mut enc = LspCodec::new(3);
        assert!(matches!(
            enc.encode(Bytes::from_static(b"1234"), &mut BytesMut::new()),
            Err(CodecError::FrameTooLarge(4))
        ));
    }

    #[test]
    fn encoding_round_trips() {
        let mut codec = LspCodec::default();
        let mut buf = BytesMut::new();
        codec.encode(Bytes::from_static(b"{\"a\":1}"), &mut buf).expect("encodes");
        assert_eq!(&buf[..], b"Content-Length: 7\r\n\r\n{\"a\":1}");
        assert_eq!(codec.decode(&mut buf).expect("decodes"), Some(Bytes::from_static(b"{\"a\":1}")));
    }
}
