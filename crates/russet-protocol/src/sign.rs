// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `sign.*`: code signing, verification and lints. Owner: W6.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.4 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `sign.run`: `{path, distribution, identityId?, profiles?{relPath: path}, mergePolicy?, output?} → SignReport`.
pub type SignRunParams = Unspecified;
/// Result of `sign.run`.
pub type SignRunResult = Unspecified;
crate::rpc_method!(
    /// `sign.run` (class `build`, long-running). W6 (Wave 1).
    SignRun,
    "sign.run",
    SignRunParams => SignRunResult,
    Permission::Build, long_running
);

/// Params of `sign.verify`: `{path, profile?: device-install|app-store} → VerifyReport`.
pub type SignVerifyParams = Unspecified;
/// Result of `sign.verify`.
pub type SignVerifyResult = Unspecified;
crate::rpc_method!(
    /// `sign.verify` (class `read`). W6 (Wave 1).
    SignVerify,
    "sign.verify",
    SignVerifyParams => SignVerifyResult,
    Permission::Read
);

/// Params of `sign.lint`: `{path, profile?: device-install|app-store} → LintFinding[]`.
pub type SignLintParams = Unspecified;
/// Result of `sign.lint`.
pub type SignLintResult = Unspecified;
crate::rpc_method!(
    /// `sign.lint` (class `read`). W6 (Wave 1).
    SignLint,
    "sign.lint",
    SignLintParams => SignLintResult,
    Permission::Read
);

/// Params of `sign.inspect`: `{path} → SignatureInfo[]`.
pub type SignInspectParams = Unspecified;
/// Result of `sign.inspect`.
pub type SignInspectResult = Unspecified;
crate::rpc_method!(
    /// `sign.inspect` (class `read`). W6 (Wave 1).
    SignInspect,
    "sign.inspect",
    SignInspectParams => SignInspectResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[SignRun::INFO, SignVerify::INFO, SignLint::INFO, SignInspect::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
