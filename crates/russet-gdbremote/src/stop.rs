// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Stop replies: `T`/`S` (stopped), `W` (exited), `X` (terminated by a signal) and `O` (inferior output), as
//! debugserver and lldb-server send them (RD §17.1; debugserver's `RNBRemote::SendStopReplyPacketForThread`).

use std::collections::BTreeMap;

use bytes::Bytes;

use crate::GdbError;
use crate::codec::{decode_payload, hex_decode, parse_hex_u64, parse_kv, parse_le_hex_u64};

/// A Mach exception carried in a stop reply (`metype`, `medata`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachException {
    pub metype: u32,
    pub medata: Vec<u64>,
}

/// Mach exception types (`metype`) the debug plane cares about.
pub mod exc {
    pub const BAD_ACCESS: u32 = 1;
    pub const BAD_INSTRUCTION: u32 = 2;
    pub const ARITHMETIC: u32 = 3;
    pub const EMULATION: u32 = 4;
    pub const SOFTWARE: u32 = 5;
    pub const BREAKPOINT: u32 = 6;
    pub const CRASH: u32 = 10;
    pub const RESOURCE: u32 = 11;
    pub const GUARD: u32 = 12;
}

/// A parsed `T`/`S` stop reply.
#[derive(Debug, Clone, PartialEq)]
pub struct StopReply {
    /// The signal number (`T11` → 0x11, SIGSTOP on Darwin).
    pub signal: u8,
    pub thread: Option<u64>,
    /// `breakpoint`, `signal`, `exception`, `trace`, `watchpoint`, …
    pub reason: Option<String>,
    /// Expedited registers, keyed by the stub's register number; values wider than eight bytes are left out.
    pub regs: BTreeMap<u32, u64>,
    pub threads: Vec<u64>,
    pub thread_pcs: Vec<u64>,
    pub mach_exception: Option<MachException>,
    /// The `jstopinfo` JSON (hex-decoded), when the stub sent it.
    pub jstopinfo: Option<serde_json::Value>,
    /// The thread name (`name` or `hexname`).
    pub name: Option<String>,
    /// The `description` (hex-decoded), for example the exception text.
    pub description: Option<String>,
    /// Every other key, in order (`qaddr`, `added-binaries`, `swbreak`, …).
    pub other: Vec<(String, String)>,
    /// The decoded payload.
    pub raw: Bytes,
}

impl StopReply {
    /// The expedited value of register `reg`.
    pub fn reg(&self, reg: u32) -> Option<u64> {
        self.regs.get(&reg).copied()
    }

    /// Whether the stop reason is a breakpoint, by `reason:breakpoint` or an `EXC_BREAKPOINT` exception.
    pub fn is_breakpoint(&self) -> bool {
        self.reason.as_deref() == Some("breakpoint")
            || self.mach_exception.as_ref().is_some_and(|e| e.metype == exc::BREAKPOINT)
    }

    /// A value from [`StopReply::other`].
    pub fn get(&self, key: &str) -> Option<&str> {
        self.other.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// What [`crate::GdbClient::wait_stop`] returns.
// A stop is moved a few times per stop; boxing it would cost an allocation per JIT stop for nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum StopEvent {
    Stopped(StopReply),
    /// `W`: the process exited with this status.
    Exited {
        code: u8,
    },
    /// `X`: the process was terminated by this signal.
    Terminated {
        signal: u8,
    },
    /// `O`: bytes the inferior wrote (lldb-server forwards a launched process's stdout this way).
    Output(Vec<u8>),
}

/// Whether a decoded payload is an `O<hex>` output packet (and not `OK`).
pub fn is_output_packet(payload: &[u8]) -> bool {
    payload.len() > 1
        && payload[0] == b'O'
        && !payload.len().is_multiple_of(2)
        && payload[1..].iter().all(u8::is_ascii_hexdigit)
}

/// Whether a decoded payload is any stop or exit reply (`T`, `S`, `W`, `X`).
pub fn is_stop_packet(payload: &[u8]) -> bool {
    match payload.first() {
        Some(b'T' | b'S') => payload.len() >= 3 && payload[1..3].iter().all(u8::is_ascii_hexdigit),
        Some(b'W' | b'X') => payload.len() >= 2 && payload[1].is_ascii_hexdigit(),
        _ => false,
    }
}

/// Parses a stop reply. `payload` may still be escaped or run-length encoded.
pub fn parse_stop(payload: &[u8]) -> Result<StopEvent, GdbError> {
    let decoded = decode_payload(payload);
    let p = decoded.as_slice();
    match p.first() {
        Some(b'T' | b'S') => {
            let signal =
                p.get(1..3).and_then(hex_decode).and_then(|v| v.first().copied()).ok_or_else(|| {
                    GdbError::Protocol(format!("malformed stop reply {:?}", String::from_utf8_lossy(p)))
                })?;
            let mut reply = StopReply {
                signal,
                thread: None,
                reason: None,
                regs: BTreeMap::new(),
                threads: Vec::new(),
                thread_pcs: Vec::new(),
                mach_exception: None,
                jstopinfo: None,
                name: None,
                description: None,
                other: Vec::new(),
                raw: Bytes::copy_from_slice(p),
            };
            if p[0] == b'T' {
                fill_t_fields(&mut reply, &p[3..]);
            }
            Ok(StopEvent::Stopped(reply))
        }
        Some(b'W') => Ok(StopEvent::Exited { code: status_byte(&p[1..]) }),
        Some(b'X') => Ok(StopEvent::Terminated { signal: status_byte(&p[1..]) }),
        Some(b'O') if is_output_packet(p) => Ok(StopEvent::Output(hex_decode(&p[1..]).unwrap_or_default())),
        Some(b'E') => Err(crate::remote_error(p)),
        None => Err(GdbError::Unsupported("empty reply where a stop reply was expected")),
        _ => Err(GdbError::Protocol(format!("not a stop reply: {:?}", String::from_utf8_lossy(p)))),
    }
}

/// `W` and `X` carry the status in hex, optionally followed by `;process:…` or `;description:…`.
fn status_byte(rest: &[u8]) -> u8 {
    let digits = rest.split(|&b| b == b';').next().unwrap_or_default();
    parse_hex_u64(digits).map(|v| (v & 0xff) as u8).unwrap_or(0)
}

/// A thread id, also in the multiprocess form `p<pid>.<tid>`.
fn parse_tid(v: &str) -> Option<u64> {
    let tid = match v.strip_prefix('p') {
        Some(rest) => rest.split_once('.').map(|(_, t)| t)?,
        None => v,
    };
    parse_hex_u64(tid.as_bytes())
}

fn parse_hex_list(v: &str) -> Vec<u64> {
    v.split(',').filter(|s| !s.is_empty()).filter_map(|s| parse_hex_u64(s.as_bytes())).collect()
}

fn hex_text(v: &str) -> String {
    match hex_decode(v.as_bytes()) {
        Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        None => v.to_owned(),
    }
}

fn fill_t_fields(reply: &mut StopReply, fields: &[u8]) {
    let mut metype = None;
    let mut medata = Vec::new();
    for (key, value) in parse_kv(fields) {
        match key.as_str() {
            "thread" => reply.thread = parse_tid(&value),
            "threads" => reply.threads = parse_hex_list(&value),
            "thread-pcs" => reply.thread_pcs = parse_hex_list(&value),
            "reason" => reply.reason = Some(value),
            "metype" => metype = parse_hex_u64(value.as_bytes()).map(|v| v as u32),
            "mecount" => {}
            "medata" => medata.extend(parse_hex_u64(value.as_bytes())),
            "jstopinfo" => {
                reply.jstopinfo = hex_decode(value.as_bytes()).and_then(|j| serde_json::from_slice(&j).ok());
            }
            "name" => reply.name = Some(value),
            "hexname" => reply.name = Some(hex_text(&value)),
            "description" => reply.description = Some(hex_text(&value)),
            k if !k.is_empty() && k.bytes().all(|b| b.is_ascii_hexdigit()) => {
                if let (Some(n), Some(v)) = (parse_hex_u64(k.as_bytes()), parse_le_hex_u64(value.as_bytes())) {
                    reply.regs.insert(n as u32, v);
                }
            }
            _ => reply.other.push((key, value)),
        }
    }
    if let Some(metype) = metype {
        reply.mach_exception = Some(MachException { metype, medata });
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn stopped(p: &[u8]) -> StopReply {
        match parse_stop(p).unwrap() {
            StopEvent::Stopped(r) => r,
            other => panic!("not stopped: {other:?}"),
        }
    }

    #[test]
    fn debugserver_breakpoint_stop_with_expedited_registers() {
        // pc (0x20) = 0x10a744174, x0 = 0x100000000, x1 = 0x4000, lr (0x1e) = 0x1000043a0.
        let p = b"T05thread:1a2b;threads:1a2b,1a2c;thread-pcs:10a744174,1a0000004;\
00:0000000001000000;01:0040000000000000;1e:a043000001000000;20:7441740a01000000;\
reason:breakpoint;qaddr:1fa0a4e0;metype:6;mecount:2;medata:1;medata:0;";
        let r = stopped(p);
        assert_eq!(r.signal, 5);
        assert_eq!(r.thread, Some(0x1a2b));
        assert_eq!(r.threads, vec![0x1a2b, 0x1a2c]);
        assert_eq!(r.thread_pcs, vec![0x10a744174, 0x1a0000004]);
        assert_eq!(r.reg(0x20), Some(0x1_0a74_4174));
        assert_eq!(r.reg(0), Some(0x1_0000_0000));
        assert_eq!(r.reg(1), Some(0x4000));
        assert_eq!(r.reg(0x1e), Some(0x1_0000_43a0));
        assert_eq!(r.reason.as_deref(), Some("breakpoint"));
        assert_eq!(r.mach_exception, Some(MachException { metype: 6, medata: vec![1, 0] }));
        assert!(r.is_breakpoint());
        assert_eq!(r.get("qaddr"), Some("1fa0a4e0"));
    }

    #[test]
    fn jstopinfo_and_hex_fields_are_decoded() {
        let json = br#"[{"tid":6699,"reason":"signal","signal":17}]"#;
        let mut p = b"T11thread:1a2b;jstopinfo:".to_vec();
        p.extend_from_slice(crate::codec::hex_encode(json).as_bytes());
        p.extend_from_slice(b";hexname:");
        p.extend_from_slice(crate::codec::hex_encode(b"main thread").as_bytes());
        p.extend_from_slice(b";description:");
        p.extend_from_slice(crate::codec::hex_encode(b"EXC_BAD_ACCESS (code=1, address=0x0)").as_bytes());
        p.push(b';');
        let r = stopped(&p);
        assert_eq!(r.signal, 0x11);
        assert_eq!(r.jstopinfo.as_ref().unwrap()[0]["tid"], 6699);
        assert_eq!(r.name.as_deref(), Some("main thread"));
        assert_eq!(r.description.as_deref(), Some("EXC_BAD_ACCESS (code=1, address=0x0)"));
        assert!(!r.is_breakpoint());
    }

    #[test]
    fn lldb_server_multiprocess_thread_ids_and_wide_registers() {
        // A 16-byte register (xmm-style) is left out; the 8-byte one stays.
        let r = stopped(b"T05thread:p3e8.3e8;name:jitdemo;10:00112233445566778899aabbccddeeff;06:1000000000000000;reason:breakpoint;");
        assert_eq!(r.thread, Some(0x3e8));
        assert_eq!(r.name.as_deref(), Some("jitdemo"));
        assert_eq!(r.reg(0x10), None);
        assert_eq!(r.reg(6), Some(0x10));
    }

    #[test]
    fn plain_signal_exit_and_termination() {
        assert_eq!(stopped(b"S11").signal, 0x11);
        assert_eq!(parse_stop(b"W00").unwrap(), StopEvent::Exited { code: 0 });
        assert_eq!(parse_stop(b"W2a;process:3e8").unwrap(), StopEvent::Exited { code: 42 });
        assert_eq!(parse_stop(b"X09").unwrap(), StopEvent::Terminated { signal: 9 });
        assert_eq!(parse_stop(b"X0b;description:73656776").unwrap(), StopEvent::Terminated { signal: 11 });
    }

    #[test]
    fn output_packets_but_not_ok() {
        assert_eq!(parse_stop(b"O68690a").unwrap(), StopEvent::Output(b"hi\n".to_vec()));
        assert!(is_output_packet(b"O68690a"));
        assert!(!is_output_packet(b"OK"));
        assert!(!is_output_packet(b"O6"));
        assert!(matches!(parse_stop(b"OK"), Err(GdbError::Protocol(_))));
    }

    #[test]
    fn errors_and_garbage() {
        assert!(matches!(parse_stop(b"E37"), Err(GdbError::Remote { code: 0x37, .. })));
        assert!(matches!(parse_stop(b""), Err(GdbError::Unsupported(_))));
        assert!(matches!(parse_stop(b"Tzz"), Err(GdbError::Protocol(_))));
        assert!(is_stop_packet(b"T05thread:1;"));
        assert!(is_stop_packet(b"W00"));
        assert!(!is_stop_packet(b"OK"));
        assert!(!is_stop_packet(b"E01"));
    }

    #[test]
    fn escaped_payloads_are_decoded_first() {
        // A register value with run-length encoding: "0* " = "0000".
        let r = stopped(b"T05thread:1;00:0* 0* 0* 0* ;");
        assert_eq!(r.reg(0), Some(0));
    }
}
