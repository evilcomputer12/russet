// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `ipc` error codes (1100–1199). Owner: W2.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 12 (daemon unavailable or version mismatch).

use crate::error::ErrorCode;

/// 1100 `ipc.protocolMismatch`: Protocol major mismatch; `details {serverProtocol, clientProtocol, restartable}`.
pub const PROTOCOL_MISMATCH: ErrorCode = ErrorCode::new(1100, "ipc.protocolMismatch", 12);
/// 1101 `ipc.notInitialized`: A request arrived before `initialize`.
pub const NOT_INITIALIZED: ErrorCode = ErrorCode::new(1101, "ipc.notInitialized", 12);
/// 1102 `ipc.daemonUnavailable`: The daemon could not be reached or spawned.
pub const DAEMON_UNAVAILABLE: ErrorCode = ErrorCode::new(1102, "ipc.daemonUnavailable", 12);
/// 1103 `ipc.daemonStarting`: The daemon is still starting.
pub const DAEMON_STARTING: ErrorCode = ErrorCode::new(1103, "ipc.daemonStarting", 12);
/// 1104 `ipc.frameTooLarge`: A frame exceeded 16 MiB; the connection is closed.
pub const FRAME_TOO_LARGE: ErrorCode = ErrorCode::new(1104, "ipc.frameTooLarge", 12);
/// 1106 `ipc.connectionClosed`: The connection closed before the reply.
pub const CONNECTION_CLOSED: ErrorCode = ErrorCode::new(1106, "ipc.connectionClosed", 12);
/// 1107 `ipc.buildMismatch`: Development builds: a different `buildId`, executable identity or device backend; `details {serverBuildId, clientBuildId, serverExecutable, restartable}` (ARCHITECTURE §3.3).
pub const BUILD_MISMATCH: ErrorCode = ErrorCode::new(1107, "ipc.buildMismatch", 12);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    PROTOCOL_MISMATCH,
    NOT_INITIALIZED,
    DAEMON_UNAVAILABLE,
    DAEMON_STARTING,
    FRAME_TOO_LARGE,
    CONNECTION_CLOSED,
    BUILD_MISMATCH,
];

/// Default hints (ARCHITECTURE §13.1), used when an error of this family carries none.
pub const HINTS: &[(ErrorCode, &str)] = &[
    (PROTOCOL_MISMATCH, "Another Russet version runs the daemon: stop it (`russet daemon restart --force`)."),
    (NOT_INITIALIZED, "Clients must send `initialize` first (ARCHITECTURE §7.4)."),
    (DAEMON_UNAVAILABLE, "Start it with `russet daemon start`, check `russet daemon logs`, or use `--no-daemon`."),
    (DAEMON_STARTING, "The daemon is starting: wait a moment and retry."),
    (FRAME_TOO_LARGE, "Messages are limited to 16 MiB: pass large data through files instead."),
    (CONNECTION_CLOSED, "The daemon went away: retry, and check `russet daemon logs` if it keeps happening."),
    (BUILD_MISMATCH, "Another build runs this daemon: `russet daemon restart --force` once its jobs are done."),
];
