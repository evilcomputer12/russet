// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet-adi-host`: the sandboxed helper that loads Apple's runtime-downloaded Android ADI libraries; no
//! network; JSON-lines IPC (ARCHITECTURE §3.1; SIGN §7.5). Layer L3 binary: F crates and russet-core only.
//!
//! The daemon spawns it as `russet-adi-host --libs <dir> --state <dir>`; it seals itself (seccomp, Landlock where
//! the kernel has it) before any Apple code is mapped, then answers one JSON line per request on stdout. It never
//! opens a socket, and nothing it handles (OTPs, machine IDs, provisioning blobs) is logged. On hosts without a
//! loader (Windows until W27c) it answers every request with an error.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::io::{BufRead as _, Write as _};
use std::process::ExitCode;

#[cfg(target_os = "linux")]
mod linux;
mod protocol;

/// Answers every request with `reason` (hosts or CPUs without a loader).
fn serve_unsupported(reason: &str) -> i32 {
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        if line.is_err() {
            break;
        }
        let resp = protocol::Response::error(reason);
        let Ok(text) = serde_json::to_string(&resp) else { break };
        if writeln!(out, "{text}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    0
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(target_os = "linux")]
    let code = linux::run(&args);
    #[cfg(not(target_os = "linux"))]
    let code = {
        let _ = &args;
        serve_unsupported("local anisette runs in Russet's Linux engine on this host")
    };
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
