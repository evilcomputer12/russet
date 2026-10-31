// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Spawning the daemon from the CLI (`daemon run --detach` from the CLI's own executable, ARCHITECTURE §3.3).
//! The platform flags (a new session on Unix; `DETACHED_PROCESS` and friends on Windows) are applied by
//! `russet_ipc::connect_or_spawn` and by `daemon run --detach` itself.

use std::ffi::OsString;
use std::time::Duration;

use russet_core::error::{Error, Result};
use russet_ipc::{DevelopmentCheck, SpawnSpec};

/// How long a client waits for a daemon it started (ARCHITECTURE §3.3 item 1).
pub const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// The spawn spec of this executable: `daemon run --detach [--idle-timeout D]`; development builds also ask
/// the running daemon to match their build, executable and device backend (ARCHITECTURE §3.3 item 3).
pub fn spawn_spec(idle_timeout: Option<Duration>) -> Result<SpawnSpec> {
    let executable = std::env::current_exe().map_err(|e| Error::io("finding the russet executable", e))?;
    let mut args: Vec<OsString> = vec!["daemon".into(), "run".into(), "--detach".into()];
    if let Some(t) = idle_timeout {
        args.push("--idle-timeout".into());
        args.push(format!("{}ms", t.as_millis()).into());
    }
    let expect =
        if russet_core::build_info::is_development() { Some(DevelopmentCheck::for_this_process()?) } else { None };
    Ok(SpawnSpec { executable, args, ready_timeout: READY_TIMEOUT, expect })
}
