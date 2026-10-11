// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Errors of the `debug` family (6000–6099, ARCHITECTURE §13.3; RD §22): constructors with their hints, and the
//! default hint of every code. `details.deviceMessage` always carries the raw device or debugserver text.

use russet_core::error::{Error, ErrorCode, codes};
use russet_gdbremote::GdbError;

/// The hint for a `debug` code (RD §22), when a next step exists.
pub fn hint(code: ErrorCode) -> Option<&'static str> {
    use codes::debug as d;
    Some(match code {
        c if c == d::LAUNCH_DENIED => {
            "Check the app's signature and entitlements (`russet sign verify`) and that the profile covers this device."
        }
        c if c == d::PROFILE_UNTRUSTED => {
            "Trust the developer on the iPhone: Settings → General → VPN & Device Management."
        }
        c if c == d::ATTACH_DENIED => {
            "Debug a Debug build signed with a development profile (get-task-allow); release and distribution builds cannot be attached."
        }
        c if c == d::ALREADY_DEBUGGED => {
            "Another debugger holds the process: stop the other session (`russet sessions`, `russet stop`) and try again."
        }
        c if c == d::DEBUGSERVER_UNAVAILABLE => {
            "Mount the Developer Disk Image (`russet device prepare`) and check `russet doctor`."
        }
        c if c == d::UUID_MISMATCH => {
            "The installed app differs from the local build: run again with `--install always`."
        }
        c if c == d::LLDB_UNSUPPORTED => {
            "Run `russet doctor`: Russet needs Swift's LLDB (or LLDB ≥ 23.1) with the remote-ios platform."
        }
        c if c == d::HANDOFF_TIMEOUT => {
            "No debugger connected in time; run `russet debug` again and keep the terminal open."
        }
        c if c == d::JIT_HOOK_NOT_FOUND => {
            "Upgrade Flutter: this engine does not export NOTIFY_DEBUGGER_ABOUT_RX_PAGES."
        }
        c if c == d::PROCESS_EXITED => {
            "The app exited before the debugger attached; see `russet logs --session` and `russet crash list`."
        }
        c if c == d::SYMBOLS_NO_SPACE => "Free disk space or prune the symbol cache (`russet symbols prune`).",
        c if c == d::BROKER_LOST => "The device or its tunnel went away: reconnect the iPhone and run again.",
        c if c == d::SESSION_NOT_FOUND => "List the sessions with `russet sessions`.",
        c if c == d::OVERLAY_UNAVAILABLE => {
            "`po` falls back to `frame variable`; see `russet doctor` for the Swift expression overlay."
        }
        c if c == d::JIT_PAGE_MISSED => "This is a Russet JIT servicer bug; please report it with `russet bugreport`.",
        _ => return None,
    })
}

/// An error with the code's default hint.
pub fn error(code: ErrorCode, msg: impl Into<String>) -> Error {
    let e = Error::new(code, msg);
    match hint(code) {
        Some(h) => e.with_hint(h),
        None => e,
    }
}

/// `debug.sessionNotFound` (6014).
pub fn session_not_found(id: &str) -> Error {
    error(codes::debug::SESSION_NOT_FOUND, format!("no session {id}"))
        .with_details(serde_json::json!({ "sessionId": id }))
}

/// Maps a failed `vAttach` to `debug.attachDenied`, `debug.alreadyDebugged` or `debug.processExited`.
pub fn attach_failed(pid: u32, e: GdbError) -> Error {
    match e {
        GdbError::Remote { code, message } => {
            let text = message.clone().unwrap_or_default();
            let lower = text.to_ascii_lowercase();
            let c = if lower.contains("already") && lower.contains("debug") {
                codes::debug::ALREADY_DEBUGGED
            } else if lower.contains("no such process") {
                codes::debug::PROCESS_EXITED
            } else {
                codes::debug::ATTACH_DENIED
            };
            error(c, format!("debugserver refused to attach to pid {pid}"))
                .with_details(serde_json::json!({ "pid": pid, "deviceMessage": text, "remoteCode": code }))
        }
        GdbError::Exited { code, signal } => {
            error(codes::debug::PROCESS_EXITED, format!("pid {pid} exited during the attach"))
                .with_details(serde_json::json!({ "pid": pid, "code": code, "signal": signal }))
        }
        other => lost(other),
    }
}

/// `debug.brokerLost` (6013) for a broken upstream connection.
pub fn lost(e: GdbError) -> Error {
    error(codes::debug::BROKER_LOST, format!("lost the connection to debugserver: {e}")).caused_by(e)
}

/// `debug.debugserverUnavailable` (6004).
pub fn debugserver_unavailable(e: impl std::fmt::Display) -> Error {
    error(codes::debug::DEBUGSERVER_UNAVAILABLE, format!("debugserver could not be reached: {e}"))
}

/// `core.io` for local socket and file trouble.
pub fn io(what: &str, e: std::io::Error) -> Error {
    Error::new(codes::core::IO, format!("{what}: {e}")).caused_by(e)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn every_user_facing_debug_code_has_a_hint() {
        for code in codes::debug::ALL {
            // jitArmingFailed and symbolsService have no next step a user can take (RD §22: "—").
            if *code == codes::debug::JIT_ARMING_FAILED || *code == codes::debug::SYMBOLS_SERVICE {
                continue;
            }
            assert!(hint(*code).is_some(), "{} has no hint", code.as_str());
        }
    }

    #[test]
    fn attach_failures_map_by_message() {
        let e = attach_failed(7, GdbError::Remote { code: 1, message: Some("no such process".into()) });
        assert_eq!(e.code(), codes::debug::PROCESS_EXITED);
        let e =
            attach_failed(7, GdbError::Remote { code: 1, message: Some("process is already being debugged".into()) });
        assert_eq!(e.code(), codes::debug::ALREADY_DEBUGGED);
        let e = attach_failed(7, GdbError::Remote { code: 1, message: Some("not allowed".into()) });
        assert_eq!(e.code(), codes::debug::ATTACH_DENIED);
        assert_eq!(e.to_data().details.unwrap()["deviceMessage"], "not allowed");
        assert_eq!(attach_failed(7, GdbError::Disconnected).code(), codes::debug::BROKER_LOST);
        assert!(e.hint().is_some());
    }
}
