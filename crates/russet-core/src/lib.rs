// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Russet core (layer L0): errors and codes, ids, directories, filesystem helpers, processes,
//! progress and event sinks, clock and entropy, endpoints, secret wrappers and the `rpc` traits.
//!
//! The public signatures follow ARCHITECTURE §5.3 (freeze F0); W2 filled the bodies (CORE-02, freeze F1).
//! Beyond §5.3 this crate holds the helpers every layer shares: [`build_info`] (the build id of ARCHITECTURE
//! §3.3), [`platform`] (uid, executable identity, TTYs), [`redact`] (log and audit redaction, CORE-13) and
//! [`task`] (supervised tasks).

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

pub mod build_info;
pub mod dirs;
pub mod error;
pub mod fsx;
pub mod ids;
pub mod net;
pub mod platform;
pub mod proc;
pub mod progress;
pub mod redact;
pub mod rpc;
pub mod secret;
pub mod task;
pub mod time;

pub use async_trait::async_trait;
pub use error::{Error, ErrorCode, ErrorData, Result};
pub use tokio_util::sync::CancellationToken;
