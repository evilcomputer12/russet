// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Russet core (layer L0): errors and codes, ids, directories, filesystem helpers, processes,
//! progress and event sinks, clock and entropy, endpoints, secret wrappers and the `rpc` traits.
//!
//! The public signatures follow ARCHITECTURE §5.3 (freeze F0). W1 landed them as compiling stubs;
//! W2 fills the bodies (CORE-02, freeze F1). `todo!()` marks a body that is not written yet.

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

pub mod dirs;
pub mod error;
pub mod fsx;
pub mod ids;
pub mod net;
pub mod proc;
pub mod progress;
pub mod rpc;
pub mod secret;
pub mod time;

pub use error::{Error, ErrorCode, ErrorData, Result};
