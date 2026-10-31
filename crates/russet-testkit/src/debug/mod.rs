// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Debug-plane test doubles (RD-02), ported from the run-debug design prototypes (Python and C evidence, now
//! deleted; RD §1 V4–V8):
//!
//! - [`fake_debugserver`]: a fake iOS debugserver (V5/V6, H6) that presents a stopped arm64 iOS process and
//!   plays scripted stops, with [`macho`]'s synthetic executable (built at test time);
//! - [`lldb_server`] and [`tools`]: `lldb-server gdbserver` from the Swift toolchain as the stub of H1–H3, and
//!   the libpython environment Swift's Linux LLDB needs;
//! - [`programs`]: the host programs, built at test time from source: `jitdemo.c` (V8, H3) and `probe.c` (a host
//!   RussetProbe with the `RUSSET_PROBE_BREAK` marker);
//! - [`dap`]: a DAP client over a child's stdio (V4, H2);
//! - [`json`]: the small JSON value these use (the testkit's manifest belongs to W8 and has no serde_json).
//!
//! Everything here is std-only and blocking; async tests call it from `spawn_blocking` or plain threads.
//! Pre-created by W1 (PLAN §3 [D 86]); W7 owns it.

pub mod dap;
pub mod fake_debugserver;
pub mod json;
pub mod lldb_server;
pub mod macho;
pub mod programs;
pub mod tools;

pub use dap::DapClient;
pub use fake_debugserver::{FakeDebugserver, FakeImage, FakeReport, FakeSpec, LostOutcome, ResumeStep};
pub use json::Json;
pub use lldb_server::LldbServer;
pub use programs::{HostProgram, PROBE_MARKER, ScratchDir};
pub use tools::LldbTools;
