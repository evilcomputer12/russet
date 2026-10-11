// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Russet contributors

//! gdb-remote client for debugserver (ARCHITECTURE §5.3 `russet-gdbremote`, RD §17.1): codec, stop-reply
//! parser, the async [`GdbClient`] and interposer hooks. It has no device knowledge, so it can go upstream.
//!
//! Layer F (MIT OR Apache-2.0). F0 stub (W1); W7 fills the bodies (RD-01).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite};

/// Errors of the gdb-remote client.
#[derive(Debug, thiserror::Error)]
pub enum GdbError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("remote error E{code:02x}")]
    Remote { code: u8, message: Option<String> },
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    #[error("disconnected")]
    Disconnected,
}

/// Handshake options.
#[derive(Debug, Clone, Default)]
pub struct HandshakeOptions {
    /// Ask for `QStartNoAckMode`.
    pub no_ack: bool,
    /// Per-packet timeout during the handshake; `None` uses the client default.
    pub timeout: Option<Duration>,
}

/// What the server reported during the handshake.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerInfo {
    pub packet_size: usize,
    pub compressions: Vec<String>,
    pub page_size: u64,
    pub os_version: Option<String>,
    pub ostype: Option<String>,
}

/// How to resume the inferior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeAction {
    ContinueAll,
    ContinueWithSignal { tid: u64, signal: u8 },
    Step { tid: u64 },
    Raw(String),
}

/// A Mach exception carried in a stop reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachException {
    pub metype: u32,
    pub medata: Vec<u64>,
}

/// A parsed `T`/`S` stop reply.
#[derive(Debug, Clone, PartialEq)]
pub struct StopReply {
    pub signal: u8,
    pub thread: Option<u64>,
    /// `breakpoint`, `signal`, `exception`, `trace`, …
    pub reason: Option<String>,
    /// Expedited registers, keyed by debugserver register number.
    pub regs: BTreeMap<u32, u64>,
    pub threads: Vec<u64>,
    pub thread_pcs: Vec<u64>,
    pub mach_exception: Option<MachException>,
    pub jstopinfo: Option<serde_json::Value>,
    pub raw: Bytes,
}

/// What [`GdbClient::wait_stop`] returns.
#[derive(Debug, Clone, PartialEq)]
pub enum StopEvent {
    Stopped(StopReply),
    Exited { code: u8 },
    Terminated { signal: u8 },
    Output(Vec<u8>),
}

/// One loaded image (`jGetLoadedDynamicLibrariesInfos`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub load_address: u64,
    pub path: String,
    pub uuid: Option<String>,
}

/// An async gdb-remote client over any byte stream.
#[derive(Debug)]
pub struct GdbClient<S> {
    _stream: PhantomData<fn() -> S>,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + 'static> GdbClient<S> {
    pub async fn handshake(stream: S, opts: HandshakeOptions) -> Result<(Self, ServerInfo), GdbError> {
        todo!("W7 (RD-01)")
    }
    pub async fn request(&mut self, payload: &[u8]) -> Result<Bytes, GdbError> {
        todo!("W7 (RD-01)")
    }
    /// Writes all packets in one syscall, then collects one reply per packet, in order.
    pub async fn pipeline(&mut self, payloads: &[Vec<u8>]) -> Result<Vec<Bytes>, GdbError> {
        todo!("W7 (RD-01)")
    }
    pub async fn attach(&mut self, pid: u32) -> Result<StopReply, GdbError> {
        todo!("W7 (RD-01)")
    }
    /// Resumes without waiting for the next stop.
    pub async fn resume(&mut self, action: ResumeAction) -> Result<(), GdbError> {
        todo!("W7 (RD-01)")
    }
    pub async fn wait_stop(&mut self) -> Result<StopEvent, GdbError> {
        todo!("W7 (RD-01)")
    }
    pub async fn interrupt(&mut self) -> Result<StopReply, GdbError> {
        todo!("W7 (RD-01)")
    }
    pub async fn read_memory(&mut self, addr: u64, len: usize) -> Result<Vec<u8>, GdbError> {
        todo!("W7 (RD-01)")
    }
    pub async fn write_memory(&mut self, addr: u64, data: &[u8]) -> Result<(), GdbError> {
        todo!("W7 (RD-01)")
    }
    /// `Z0`.
    pub async fn set_breakpoint(&mut self, addr: u64, kind: u8) -> Result<(), GdbError> {
        todo!("W7 (RD-01)")
    }
    /// `z0`.
    pub async fn remove_breakpoint(&mut self, addr: u64, kind: u8) -> Result<(), GdbError> {
        todo!("W7 (RD-01)")
    }
    pub async fn write_register(&mut self, tid: u64, reg: u32, value: u64) -> Result<(), GdbError> {
        todo!("W7 (RD-01)")
    }
    pub async fn loaded_images(&mut self) -> Result<Vec<ImageInfo>, GdbError> {
        todo!("W7 (RD-01)")
    }
    pub async fn detach(self) -> Result<(), GdbError> {
        todo!("W7 (RD-01)")
    }
    pub async fn kill(self) -> Result<Option<StopEvent>, GdbError> {
        todo!("W7 (RD-01)")
    }
}
