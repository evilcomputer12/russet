// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russetd`, the per-user daemon (ARCHITECTURE §3, §5.3 `russetd`, §6): lifecycle, [`DaemonCtx`], handlers,
//! pipelines, permissions, approvals, jobs, leases, subscriptions, the web endpoint and the audit log.
//!
//! F0 stub (W1); W2 owns this file and the runtime modules (CORE-06, freeze F1).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::time::Duration;

use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_ipc::{Endpoint, Peer};

pub mod approvals;
pub mod audit;
pub mod ctx;
pub mod daemon;
pub mod doctor_registry;
pub mod handlers;
pub mod jobs;
pub mod leases;
pub mod perms;
pub mod pipeline;
pub mod state;
pub mod subs;
pub mod web;

pub use ctx::{DaemonCtx, DeviceSlot};

/// How to run the daemon (`russet daemon run`).
#[derive(Debug)]
pub struct DaemonOptions {
    pub dirs: RussetDirs,
    pub endpoint: Endpoint,
    /// `daemon.idle-timeout` (default 30 min); `None` never idles out.
    pub idle_timeout: Option<Duration>,
    pub foreground: bool,
    /// `--allow-live-device`: development builds otherwise refuse the `live` backend without `RUSSET_LANE=A`
    /// (ARCHITECTURE §3.3 item 7).
    pub allow_live_device: bool,
}

/// Runs the daemon until shutdown.
pub async fn run(opts: DaemonOptions) -> Result<()> {
    todo!("W2 (CORE-06)")
}

/// An in-process daemon: `--no-daemon` and tests.
#[derive(Debug)]
pub struct InProcess {
    _f0: (),
}

impl InProcess {
    pub async fn start(o: DaemonOptions) -> Result<(InProcess, Peer)> {
        todo!("W2 (CORE-06)")
    }
    pub async fn stop(self) -> Result<()> {
        todo!("W2 (CORE-06)")
    }
}
