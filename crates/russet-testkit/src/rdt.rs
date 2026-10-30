// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The Russet Device Trace (RDT) format v1 (QA §3.5.2): the on-disk record of a device session, recorded by
//! [`crate::RecordingConnector`] and played by [`crate::ReplayConnector`].
//!
//! A trace is JSON Lines. The first line is a [`Header`]; every other line is a [`Record`]. The canonical
//! on-disk form is zstd-compressed (`*.rdt.jsonl.zst`), but a plain `*.rdt.jsonl` is read and written too, so
//! committed fixtures stay reviewable and `cargo xtask traces` (a seam crate that cannot depend on zstd) can
//! lint them. [`Trace::read`] and [`Trace::write`] pick the form from the path's extension.
//!
//! Records (the `type` discriminator):
//!
//! - `header` — one, first: tool versions, the host OS, the device identity and the redaction policy.
//! - `conn` — a service stream opening (`open: true`, with `via` and `service`) or closing (`open: false`).
//! - `data` — bytes on an open stream, base64, with a direction (`c2d` client→device, `d2c` device→client).
//! - `call` — a facade call ([`russet_device::io::FacadeCall`]) with its result or error.
//! - `note` — an optional decoded view of a protocol message, for human review and drift diffs; ignored on replay.
//! - `elided` — a payload the recorder refused to store (an Apple file), kept as a length and a hash (QA §3.5.2).
//!
//! Elision and redaction keep Apple payloads and personal identifiers out of committed traces (QA §3.5.3,
//! [`crate::redaction`]).

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use russet_core::error::{Error, Result, codes};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The RDT format version this testkit reads and writes.
pub const VERSION: u32 = 1;

/// A trace header (QA §3.5.2). The first line of every trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    /// The format version; [`VERSION`].
    pub rdt: u32,
    /// The scenario name, for `cargo xtask traces` and the file layout.
    pub scenario: String,
    /// RFC 3339 UTC. Fixed in recordings made from a scripted scenario, so fixtures are reproducible (QA §2 T10).
    pub recorded_at: String,
    /// The Russet version that recorded it.
    pub russet: String,
    /// The idevice version that recorded it.
    pub idevice: String,
    /// `linux`, `windows` or `macos`.
    pub host_os: String,
    /// The recorded device's identity (redacted; QA §3.5.3).
    pub device: Device,
    /// The redaction policy applied (`default-v1`).
    pub redaction: Redaction,
}

/// The device a trace was recorded against (QA §3.5.2 `device`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub product_type: String,
    pub product_version: String,
    pub build_version: String,
    /// `txm`, `ppl`, `none` or `unknown` (ARCHITECTURE §9.5): the device's code-signing monitor.
    pub csm: String,
    /// A synthetic UDID (chip id `0000`, never a real device; QA §3.5.3).
    pub udid: String,
    /// `usb` or `wifi`.
    #[serde(default = "usb")]
    pub transport: String,
}

fn usb() -> String {
    "usb".into()
}

/// The redaction policy a trace declares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Redaction {
    /// `default-v1`.
    pub policy: String,
}

impl Default for Redaction {
    fn default() -> Self {
        Self { policy: crate::redaction::POLICY.into() }
    }
}

/// The direction of a `data` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// Client → device (what Russet sent).
    C2d,
    /// Device → client (what the device sent).
    D2c,
}

/// Which seam a stream was opened through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    Lockdown,
    Rsd,
}

/// A facade call's recorded outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallError {
    /// The `russetCode` (ARCHITECTURE §13.3), for example `device.serviceNotImplemented`.
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// One record after the header. The `type` field selects the variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Record {
    /// A stream opening or closing.
    Conn {
        id: u64,
        t: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        via: Option<Via>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        service: Option<String>,
        open: bool,
    },
    /// Bytes on a stream.
    Data { conn: u64, t: f64, dir: Dir, b64: String },
    /// A facade call and its answer.
    Call {
        t: f64,
        api: String,
        #[serde(default)]
        args: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<CallError>,
    },
    /// A decoded protocol view (human-only; ignored on replay).
    Note {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conn: Option<u64>,
        t: f64,
        proto: String,
        decoded: Value,
    },
    /// A payload the recorder refused to store (an Apple file).
    Elided {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conn: Option<u64>,
        t: f64,
        dir: Dir,
        len: u64,
        sha256: String,
        reason: String,
    },
}

/// A whole trace: a header and its records, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub header: Header,
    pub records: Vec<Record>,
}

impl Trace {
    /// A new, empty trace with `header`.
    pub fn new(header: Header) -> Self {
        Self { header, records: Vec::new() }
    }

    /// Parses a trace from its JSON Lines bytes (already decompressed).
    pub fn from_jsonl(bytes: &[u8]) -> Result<Self> {
        let mut lines = BufReader::new(bytes).lines();
        let first = lines
            .next()
            .transpose()
            .map_err(read_err)?
            .ok_or_else(|| format_err("the trace is empty (no header line)"))?;
        let header: Header = serde_json::from_str(&first).map_err(|e| format_err(format!("bad header: {e}")))?;
        if header.rdt != VERSION {
            return Err(format_err(format!("RDT version {} (this testkit reads version {VERSION})", header.rdt)));
        }
        let mut records = Vec::new();
        for (i, line) in lines.enumerate() {
            let line = line.map_err(read_err)?;
            if line.trim().is_empty() {
                continue;
            }
            let rec: Record =
                serde_json::from_str(&line).map_err(|e| format_err(format!("bad record on line {}: {e}", i + 2)))?;
            records.push(rec);
        }
        Ok(Self { header, records })
    }

    /// The trace as JSON Lines bytes (uncompressed), one record per line, trailing newline.
    pub fn to_jsonl(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut w = BufWriter::new(&mut out);
        serde_json::to_writer(&mut w, &self.header).map_err(ser_err)?;
        w.write_all(b"\n").map_err(read_err)?;
        for rec in &self.records {
            serde_json::to_writer(&mut w, rec).map_err(ser_err)?;
            w.write_all(b"\n").map_err(read_err)?;
        }
        w.flush().map_err(read_err)?;
        drop(w);
        Ok(out)
    }

    /// Reads a trace from a `*.rdt.jsonl` or `*.rdt.jsonl.zst` file (by extension).
    pub fn read(path: &Path) -> Result<Self> {
        let raw = std::fs::read(path).map_err(|e| {
            Error::new(codes::core::NOT_FOUND, format!("cannot read the trace {}: {e}", path.display()))
        })?;
        let bytes = if is_zst(path) {
            zstd::decode_all(&raw[..]).map_err(|e| format_err(format!("{}: not valid zstd: {e}", path.display())))?
        } else {
            raw
        };
        Self::from_jsonl(&bytes).map_err(|e| format_err(format!("{}: {}", path.display(), e.message())))
    }

    /// Writes a trace to a `*.rdt.jsonl` or `*.rdt.jsonl.zst` file (by extension). Creates parent directories.
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(read_err)?;
        }
        let jsonl = self.to_jsonl()?;
        let bytes = if is_zst(path) {
            zstd::encode_all(&jsonl[..], 19).map_err(|e| format_err(format!("zstd encode: {e}")))?
        } else {
            jsonl
        };
        std::fs::write(path, bytes).map_err(|e| format_err(format!("cannot write the trace {}: {e}", path.display())))
    }

    /// The device summary this trace replays (QA §3.5): built from the header.
    pub fn device_summary(&self) -> russet_device::DeviceSummary {
        let transport = if self.header.device.transport == "wifi" {
            russet_device::model::Transport::Wifi
        } else {
            russet_device::model::Transport::Usb
        };
        russet_device::DeviceSummary::attached(russet_device::Udid(self.header.device.udid.clone()), transport)
    }
}

fn is_zst(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zst"))
}

fn format_err(msg: impl Into<String>) -> Error {
    Error::new(codes::core::INVALID_ARGUMENT, format!("RDT trace: {}", msg.into()))
}

fn read_err(e: std::io::Error) -> Error {
    Error::new(codes::core::IO, format!("RDT trace I/O: {e}"))
}

fn ser_err(e: serde_json::Error) -> Error {
    Error::new(codes::core::INVALID_ARGUMENT, format!("RDT serialize: {e}"))
}

/// Standard base64 (no padding needed to decode; [`crate::scenario::base64_decode`] reads both forms).
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[(n & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            rdt: VERSION,
            scenario: "device-info".into(),
            recorded_at: "2026-10-11T00:00:00Z".into(),
            russet: "0.1.0".into(),
            idevice: "0.1.68".into(),
            host_os: "linux".into(),
            device: Device {
                product_type: "iPhone13,2".into(),
                product_version: "27.0".into(),
                build_version: "24A437".into(),
                csm: "txm".into(),
                udid: "00000000-0000000000000001".into(),
                transport: "usb".into(),
            },
            redaction: Redaction::default(),
        }
    }

    #[test]
    fn base64_round_trips() {
        for case in [&b""[..], b"\x00\x01\x02", b"hello", b"pong\n\x00\x01\x02", &[0xde, 0xad, 0xbe, 0xef]] {
            let enc = base64_encode(case);
            let dec = crate::scenario::base64_decode(&enc).expect("decode");
            assert_eq!(dec, case, "{enc}");
        }
    }

    #[test]
    fn jsonl_round_trips() {
        let mut t = Trace::new(header());
        t.records.push(Record::Conn {
            id: 1,
            t: 0.1,
            via: Some(Via::Lockdown),
            service: Some("com.apple.syslog_relay".into()),
            open: true,
        });
        t.records.push(Record::Data { conn: 1, t: 0.2, dir: Dir::D2c, b64: base64_encode(b"line\n\0") });
        t.records.push(Record::Call {
            t: 0.3,
            api: "lockdown.get_value".into(),
            args: serde_json::json!({ "key": "ProductVersion" }),
            result: Some(serde_json::json!("27.0")),
            error: None,
        });
        t.records.push(Record::Conn { id: 1, t: 0.4, via: None, service: None, open: false });
        let bytes = t.to_jsonl().expect("serialize");
        let back = Trace::from_jsonl(&bytes).expect("parse");
        assert_eq!(back, t);
    }

    #[test]
    fn a_version_mismatch_is_rejected() {
        let mut h = header();
        h.rdt = 99;
        let t = Trace::new(h);
        let bytes = t.to_jsonl().expect("serialize");
        let e = Trace::from_jsonl(&bytes).expect_err("version");
        assert_eq!(e.code(), codes::core::INVALID_ARGUMENT);
        assert!(e.message().contains("version 99"), "{}", e.message());
    }
}
