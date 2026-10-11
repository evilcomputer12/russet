// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Shared helpers of russetd's integration tests (included with `#[path]`; on its own it has no tests): a
//! temporary `RUSSET_HOME` per test (ARCHITECTURE §3.3 item 7) and in-process daemons with clients of each
//! kind.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use russet_core::dirs::RussetDirs;
use russet_core::fsx::TempDir;
use russet_core::rpc::ClientKind;
use russet_ipc::{ClientInfo, Peer};
use russetd::{DaemonOptions, InProcess};

/// A temporary home and the options of a daemon living in it (no device plane, no live lock).
pub struct Home {
    pub dir: TempDir,
    pub dirs: RussetDirs,
}

impl Home {
    pub fn new(name: &str) -> Self {
        let dir = TempDir::new(name).expect("temp home");
        let dirs = RussetDirs::under(dir.path());
        Self { dir, dirs }
    }

    /// Default options; the live-device lock path is inside the home, never the real one.
    pub fn options(&self) -> DaemonOptions {
        let mut o = DaemonOptions::new(self.dirs.clone());
        o.live_device_lock = self.dir.path().join("live-device.lock");
        o.home = Some(self.dir.path().to_path_buf());
        o
    }
}

/// A client description of `kind` named `name` with the given TTY and confirmation capabilities.
pub fn client(kind: ClientKind, name: &str, interactive: bool) -> ClientInfo {
    let mut c = ClientInfo::new(kind, name);
    c.capabilities.interactive = interactive;
    c.capabilities.confirm = interactive;
    c.capabilities.prompt = interactive;
    c.capabilities.approvals = kind == ClientKind::Ide || interactive;
    c
}

/// Starts an in-process daemon with `opts` and connects a CLI client with a TTY.
pub async fn start(opts: DaemonOptions) -> (InProcess, Peer) {
    let (d, peer, _) = InProcess::start_with(opts, &client(ClientKind::Cli, "test-cli", true)).await.expect("daemon");
    (d, peer)
}
