// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The russet/1 framing (ARCHITECTURE §7.2): `Content-Length: <n>\r\n\r\n<n bytes of UTF-8 JSON-RPC 2.0>`.
//! Header names are case-insensitive, `Content-Type` is ignored, frames are at most 16 MiB.
//!
//! This is the testkit's own small codec, independent of russet-ipc's, so tests that drive the daemon through
//! it also check W2's codec against a second implementation.

use std::io;

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest frame (ARCHITECTURE §7.2).
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Writes one message.
pub async fn write_message<W: AsyncWrite + Unpin>(w: &mut W, msg: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    w.write_all(header.as_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

/// Reads one message; `Ok(None)` at a clean end of stream (before any header byte).
pub async fn read_message<R: AsyncBufRead + Unpin>(r: &mut R) -> io::Result<Option<Value>> {
    let mut length: Option<usize> = None;
    let mut first = true;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).await?;
        if n == 0 {
            if first {
                return Ok(None);
            }
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "end of stream inside a frame header"));
        }
        first = false;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad header line {line:?}")));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let v: usize = value
                .trim()
                .parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("bad Content-Length {value:?}")))?;
            length = Some(v);
        }
    }
    let len = length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a frame without Content-Length"))?;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {len} bytes exceeds 16 MiB")));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The raw bytes of one frame, for tests that split or corrupt frames on purpose.
pub fn frame_bytes(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(msg).unwrap_or_default();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}
