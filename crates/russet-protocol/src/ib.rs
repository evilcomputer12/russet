// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `ib.*`: the storyboard and XIB compiler behind the `ibtool` shim [D 75]. Owner: W21b (Wave 4), W28b and W28c (Wave 6).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.3 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `ib.compile`: `{input: .storyboard|.xib, outDir, module?, deploymentTarget, devices: (iphone|ipad)[], mode?: compile|link} → {outputs[], partialInfoPlist?, diagnostics[]}`.
pub type IbCompileParams = Unspecified;
/// Result of `ib.compile`.
pub type IbCompileResult = Unspecified;
crate::rpc_method!(
    /// `ib.compile` (class `build`). W21b / W28b / W28c (Waves 4 and 6).
    IbCompile,
    "ib.compile",
    IbCompileParams => IbCompileResult,
    Permission::Build
);

/// Params of `ib.dump`: `{path: .nib|.storyboardc} → {graph}`.
pub type IbDumpParams = Unspecified;
/// Result of `ib.dump`.
pub type IbDumpResult = Unspecified;
crate::rpc_method!(
    /// `ib.dump` (class `read`). W21b (Wave 4).
    IbDump,
    "ib.dump",
    IbDumpParams => IbDumpResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[IbCompile::INFO, IbDump::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
