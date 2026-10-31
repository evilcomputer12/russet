// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `core` error codes (1000–1099). Owner: W2.
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 1 (unexpected failure), except `core.notFound` (3), `core.diskFull` (4) and `core.cancelled` (130).

use crate::error::ErrorCode;

/// 1000 `core.internal`: An unexpected internal failure.
pub const INTERNAL: ErrorCode = ErrorCode::new(1000, "core.internal", 1);
/// 1001 `core.cancelled`: The operation was cancelled (`$/cancelRequest`, SIGINT); JSON-RPC answers `-32800`.
pub const CANCELLED: ErrorCode = ErrorCode::new(1001, "core.cancelled", 130);
/// 1002 `core.timeout`: The operation timed out.
pub const TIMEOUT: ErrorCode = ErrorCode::new(1002, "core.timeout", 1);
/// 1003 `core.io`: A filesystem or I/O operation failed.
pub const IO: ErrorCode = ErrorCode::new(1003, "core.io", 1);
/// 1004 `core.diskFull`: Not enough free space (`fsx::ensure_space`, `storage.min-free-space`).
pub const DISK_FULL: ErrorCode = ErrorCode::new(1004, "core.diskFull", 4);
/// 1005 `core.notImplemented`: The method or command is registered but not implemented yet (F0 stubs).
pub const NOT_IMPLEMENTED: ErrorCode = ErrorCode::new(1005, "core.notImplemented", 1);
/// 1006 `core.invalidArgument`: A parameter failed validation.
pub const INVALID_ARGUMENT: ErrorCode = ErrorCode::new(1006, "core.invalidArgument", 1);
/// 1007 `core.notFound`: The named object does not exist.
pub const NOT_FOUND: ErrorCode = ErrorCode::new(1007, "core.notFound", 3);
/// 1008 `core.busy`: A conflicting job holds the lock (`LockKey`).
pub const BUSY: ErrorCode = ErrorCode::new(1008, "core.busy", 1);
/// 1009 `core.unsupportedPlatform`: Not supported on this host platform.
pub const UNSUPPORTED_PLATFORM: ErrorCode = ErrorCode::new(1009, "core.unsupportedPlatform", 1);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    INTERNAL,
    CANCELLED,
    TIMEOUT,
    IO,
    DISK_FULL,
    NOT_IMPLEMENTED,
    INVALID_ARGUMENT,
    NOT_FOUND,
    BUSY,
    UNSUPPORTED_PLATFORM,
];

/// Default hints (ARCHITECTURE §13.1), used when an error of this family carries none. `core.cancelled` has no
/// next step beyond running the command again.
pub const HINTS: &[(ErrorCode, &str)] = &[
    (INTERNAL, "This is a bug in Russet: run `russet bugreport` and attach the archive to an issue."),
    (TIMEOUT, "Retry; if it keeps timing out, run `russet doctor` and check `russet daemon logs`."),
    (IO, "Check the path's permissions and free space; `russet daemon logs` has the details."),
    (DISK_FULL, "Free some space (`russet toolchain gc`, `russet sdk remove`, old caches) and retry."),
    (NOT_IMPLEMENTED, "This lands in a later workstream (docs/PLAN.md); see ARCHITECTURE §8 for its wave."),
    (INVALID_ARGUMENT, "Check the arguments (`russet <command> --help`)."),
    (NOT_FOUND, "Check the id or name; the matching `list` command shows what exists."),
    (BUSY, "Another job holds the lock: wait for it (`russet daemon status`) or cancel it."),
    (UNSUPPORTED_PLATFORM, "This feature is not available on this platform yet (ARCHITECTURE §18)."),
];
