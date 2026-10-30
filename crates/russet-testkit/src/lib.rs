// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Test doubles for every testability contract (QA §2): scripted, replay and recording `ServiceConnector`s,
//! a mock usbmuxd, cassettes, HTTP fakes, the temp-home daemon fixture (`TempHome`) and RDT traces.
//!
//! Tests of an L3 crate that use the testkit are integration tests under that crate's `tests/`, never
//! `#[cfg(test)]` unit modules: the testkit ↔ crate dev-dependency cycle would otherwise compile two copies of
//! the crate's types (ARCHITECTURE §5.1).
//!
//! What Wave 1 provides (CORE-15, PLAN W8):
//!
//! | Module | For |
//! |---|---|
//! | [`home`] | [`TempHome`]: a fresh `RUSSET_HOME` per test with the lane-D environment (memory secrets, a private live-device lock, offline HTTP, a scripted or `none` device backend), and the env for spawned processes |
//! | [`scenario`], [`scripted`] | [`ScriptedConnector`] (core's FakeDeviceBackend, [D 46]): scenario TOML files drive attach and detach, facade calls, scripted service streams, syslog lines and failures, with a [`Control`] handle for tests |
//! | [`daemon`] | [`SpawnedDaemon`]: `russet daemon run` in a `TempHome`, readiness, logs and teardown |
//! | [`tripwire`] | [`Tripwire`]: a `USBMUXD_SOCKET_ADDRESS` socket that records any connection (the lane-D isolation test) |
//! | [`lsp`], [`transcript`] | russet/1 framing (ARCHITECTURE §7.2) and JSON-RPC transcript replay with captures and wildcards |
//! | [`fake_daemon`] | a tiny scripted russet/1 peer, to test clients and harnesses without the daemon |
//! | [`node`] | the Node interop harness (vscode-jsonrpc 9.0.3) shared with W2 |
//!
//! What Wave 2 adds (QA-06 to QA-09, PLAN W15):
//!
//! | Module | For |
//! |---|---|
//! | [`rdt`] | the Russet Device Trace format v1 (QA §3.5.2): the on-disk record of a device session |
//! | [`replay`] | [`ReplayConnector`]: serves a recorded [`rdt::Trace`] as a `ServiceConnector`, no device, no sockets |
//! | [`recording`] | [`RecordingConnector`]: wraps any connector and records a trace of its traffic (tapped streams) |
//! | [`redaction`] | the `default-v1` redaction policy and the trace lint (QA §3.5.3), shared with `cargo xtask traces` |
//!
//! The out-of-process mock device (`russet-mockdevice`), cassettes and HTTP fakes follow. F0 stub by W1; W8
//! owns the crate, W7 owns [`debug`], W15 owns the rest.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

/// Debug-plane test doubles: the fake debugserver, the pre-attach relay and the JIT demo harness, ported from
/// `docs/design/run-debug-prototypes` (RD-02). Pre-created for W7.
pub mod debug;

pub mod daemon;
pub mod fake_daemon;
pub mod home;
pub mod lsp;
pub mod node;
pub mod rdt;
pub mod recording;
pub mod redaction;
pub mod replay;
pub mod scenario;
pub mod scripted;
pub mod transcript;
pub mod tripwire;

pub use daemon::SpawnedDaemon;
pub use home::{Backend, TempHome};
pub use recording::RecordingConnector;
pub use replay::{ReplayConnector, ReplayError, ReplayMode};
pub use scenario::Scenario;
pub use scripted::{Control, ScriptedConnector};
pub use transcript::Transcript;
pub use tripwire::Tripwire;

/// The testkit's own directory (scenarios, the Node harness), from its manifest.
pub fn testkit_dir() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
}
