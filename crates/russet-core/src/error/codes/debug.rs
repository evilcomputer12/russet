// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//! `debug` error codes (6000–6099). Owner: W7, W11, W18 (6016).
//!
//! Pre-created by W1 with every constant of ARCHITECTURE §13.3 (freeze F0). Add codes only in this
//! family's range, append them to [`ALL`], and give each user-visible error a hint and a docs anchor.
//!
//! Exit 9 (launch or debug failed).

use crate::error::ErrorCode;

/// 6000 `debug.launchDenied`: The device refused the debug launch.
pub const LAUNCH_DENIED: ErrorCode = ErrorCode::new(6000, "debug.launchDenied", 9);
/// 6001 `debug.profileUntrusted`: The developer profile is not trusted on the device.
pub const PROFILE_UNTRUSTED: ErrorCode = ErrorCode::new(6001, "debug.profileUntrusted", 9);
/// 6002 `debug.attachDenied`: The device refused the attach.
pub const ATTACH_DENIED: ErrorCode = ErrorCode::new(6002, "debug.attachDenied", 9);
/// 6003 `debug.alreadyDebugged`: The process already has a debugger.
pub const ALREADY_DEBUGGED: ErrorCode = ErrorCode::new(6003, "debug.alreadyDebugged", 9);
/// 6004 `debug.debugserverUnavailable`: debugserver could not be reached.
pub const DEBUGSERVER_UNAVAILABLE: ErrorCode = ErrorCode::new(6004, "debug.debugserverUnavailable", 9);
/// 6005 `debug.uuidMismatch`: The local binary's UUID does not match the installed app.
pub const UUID_MISMATCH: ErrorCode = ErrorCode::new(6005, "debug.uuidMismatch", 9);
/// 6006 `debug.lldbUnsupported`: The LLDB version or flavor is unsupported.
pub const LLDB_UNSUPPORTED: ErrorCode = ErrorCode::new(6006, "debug.lldbUnsupported", 9);
/// 6007 `debug.handoffTimeout`: LLDB did not take over the broker in time.
pub const HANDOFF_TIMEOUT: ErrorCode = ErrorCode::new(6007, "debug.handoffTimeout", 9);
/// 6008 `debug.jitHookNotFound`: The JIT hook symbol was not found.
pub const JIT_HOOK_NOT_FOUND: ErrorCode = ErrorCode::new(6008, "debug.jitHookNotFound", 9);
/// 6009 `debug.jitArmingFailed`: Arming the JIT hook failed.
pub const JIT_ARMING_FAILED: ErrorCode = ErrorCode::new(6009, "debug.jitArmingFailed", 9);
/// 6010 `debug.processExited`: The process exited.
pub const PROCESS_EXITED: ErrorCode = ErrorCode::new(6010, "debug.processExited", 9);
/// 6011 `debug.symbolsNoSpace`: Not enough space or quota for device symbols.
pub const SYMBOLS_NO_SPACE: ErrorCode = ErrorCode::new(6011, "debug.symbolsNoSpace", 9);
/// 6012 `debug.symbolsService`: The device symbols service failed.
pub const SYMBOLS_SERVICE: ErrorCode = ErrorCode::new(6012, "debug.symbolsService", 9);
/// 6013 `debug.brokerLost`: The debug broker lost its connection.
pub const BROKER_LOST: ErrorCode = ErrorCode::new(6013, "debug.brokerLost", 9);
/// 6014 `debug.sessionNotFound`: No session with this id.
pub const SESSION_NOT_FOUND: ErrorCode = ErrorCode::new(6014, "debug.sessionNotFound", 9);
/// 6015 `debug.overlayUnavailable`: The LLDB Swift overlay is unavailable (RD §22).
pub const OVERLAY_UNAVAILABLE: ErrorCode = ErrorCode::new(6015, "debug.overlayUnavailable", 9);
/// 6016 `debug.jitPageMissed`: A crash at a page that was announced but not touched, or in an executable region never announced: a servicer bug; `details {region, page, pc, faultAddress}`.
pub const JIT_PAGE_MISSED: ErrorCode = ErrorCode::new(6016, "debug.jitPageMissed", 9);
/// 6017 `debug.batchTimeout`: `russet debug --batch` did not stop and finish its commands within `--timeout`
/// (W7; `details {stopped, timeoutSecs, result}`).
pub const BATCH_TIMEOUT: ErrorCode = ErrorCode::new(6017, "debug.batchTimeout", 9);

/// Every code of this family, in number order.
pub const ALL: &[ErrorCode] = &[
    LAUNCH_DENIED,
    PROFILE_UNTRUSTED,
    ATTACH_DENIED,
    ALREADY_DEBUGGED,
    DEBUGSERVER_UNAVAILABLE,
    UUID_MISMATCH,
    LLDB_UNSUPPORTED,
    HANDOFF_TIMEOUT,
    JIT_HOOK_NOT_FOUND,
    JIT_ARMING_FAILED,
    PROCESS_EXITED,
    SYMBOLS_NO_SPACE,
    SYMBOLS_SERVICE,
    BROKER_LOST,
    SESSION_NOT_FOUND,
    OVERLAY_UNAVAILABLE,
    JIT_PAGE_MISSED,
    BATCH_TIMEOUT,
];
