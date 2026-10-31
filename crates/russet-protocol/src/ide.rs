// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `ide.*`: requests the daemon sends to `ide` connections that declared `capabilities.ideDebug` [D 36]. Owner: W11 (daemon side), W14 (extension side).
//!
//! F0 stub (W1): every name of ARCHITECTURE §9.4 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `ide.debug.start`: `{configuration: RussetDebugConfiguration} → {sessionId}`.
pub type IdeDebugStartParams = Unspecified;
/// Result of `ide.debug.start`.
pub type IdeDebugStartResult = Unspecified;
crate::rpc_method!(
    /// `ide.debug.start` (served by the client). W11.
    IdeDebugStart,
    "ide.debug.start",
    IdeDebugStartParams => IdeDebugStartResult,
    Permission::None
);

/// Params of `ide.breakpoints.add`: `{file, line, condition?, logMessage?} → {id}`.
pub type IdeBreakpointsAddParams = Unspecified;
/// Result of `ide.breakpoints.add`.
pub type IdeBreakpointsAddResult = Unspecified;
crate::rpc_method!(
    /// `ide.breakpoints.add` (served by the client). W11.
    IdeBreakpointsAdd,
    "ide.breakpoints.add",
    IdeBreakpointsAddParams => IdeBreakpointsAddResult,
    Permission::None
);

/// Params of `ide.breakpoints.remove`: `{id} → {}`.
pub type IdeBreakpointsRemoveParams = Unspecified;
/// Result of `ide.breakpoints.remove`.
pub type IdeBreakpointsRemoveResult = Unspecified;
crate::rpc_method!(
    /// `ide.breakpoints.remove` (served by the client). W11.
    IdeBreakpointsRemove,
    "ide.breakpoints.remove",
    IdeBreakpointsRemoveParams => IdeBreakpointsRemoveResult,
    Permission::None
);

/// Params of `ide.reveal`: `{file, line?, column?} → {}`.
pub type IdeRevealParams = Unspecified;
/// Result of `ide.reveal`.
pub type IdeRevealResult = Unspecified;
crate::rpc_method!(
    /// `ide.reveal` (served by the client). W11.
    IdeReveal,
    "ide.reveal",
    IdeRevealParams => IdeRevealResult,
    Permission::None
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] =
    &[IdeDebugStart::INFO, IdeBreakpointsAdd::INFO, IdeBreakpointsRemove::INFO, IdeReveal::INFO];
