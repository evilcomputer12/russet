// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Russet contributors

//! gdb-remote client for debugserver (ARCHITECTURE §5.3 `russet-gdbremote`, RD §17.1): the [`codec`], the
//! [`stop`]-reply parser, the async [`GdbClient`], the [`interpose`] relay with hooks and the [`arm64`]
//! helpers. It has no device knowledge, so it can go upstream.
//!
//! Layer F (MIT OR Apache-2.0). Owner: W7 (RD-01).

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

pub mod arm64;
pub mod client;
pub mod codec;
pub mod interpose;
pub mod stop;

pub use client::{
    ClientStats, GdbClient, HandshakeOptions, ImageInfo, MemoryRegion, ProcessInfo, RawSession, RegisterInfo,
    ResumeAction, SegmentInfo, ServerInfo, SharedCacheInfo, check_ok, memory_write_packet,
};
pub use stop::{MachException, StopEvent, StopReply, parse_stop, parse_stop_raw};

/// Errors of the gdb-remote client.
#[derive(Debug, thiserror::Error)]
pub enum GdbError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("remote error E{code:02x}{}", message.as_deref().map(|m| format!(": {m}")).unwrap_or_default())]
    Remote { code: u8, message: Option<String> },
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    #[error("disconnected")]
    Disconnected,
    /// The process exited (or was killed) where a stop was expected.
    #[error("the process exited (code {code:?}, signal {signal:?})")]
    Exited { code: Option<u8>, signal: Option<u8> },
}

/// Parses an error reply: `Exx`, `Exx;<hex message>` (after `QEnableErrorStrings`) or `E.<text>`.
pub fn remote_error(reply: &[u8]) -> GdbError {
    let rest = reply.strip_prefix(b"E").unwrap_or(reply);
    if let Some(text) = rest.strip_prefix(b".") {
        return GdbError::Remote { code: 0, message: Some(String::from_utf8_lossy(text).into_owned()) };
    }
    let (digits, message) = match rest.iter().position(|&b| b == b';') {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let code = codec::parse_hex_u64(digits).map(|v| (v & 0xff) as u8).unwrap_or(0);
    let message = message.map(|m| match codec::hex_decode(m) {
        Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        None => String::from_utf8_lossy(m).into_owned(),
    });
    GdbError::Remote { code, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_replies() {
        assert!(matches!(remote_error(b"E08"), GdbError::Remote { code: 8, message: None }));
        let msg = format!("E16;{}", codec::hex_encode(b"russet broker already attached pid 932"));
        match remote_error(msg.as_bytes()) {
            GdbError::Remote { code: 0x16, message: Some(m) } => {
                assert_eq!(m, "russet broker already attached pid 932")
            }
            other => panic!("{other:?}"),
        }
        match remote_error(b"E.operation not permitted") {
            GdbError::Remote { code: 0, message: Some(m) } => assert_eq!(m, "operation not permitted"),
            other => panic!("{other:?}"),
        }
        assert_eq!(remote_error(b"E09").to_string(), "remote error E09");
    }
}
