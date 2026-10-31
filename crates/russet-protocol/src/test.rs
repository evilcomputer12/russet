// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `test.*`: XCTest on device [D 80]. Owner: W32 (Wave 4), W28 (Wave 6).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.5 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `test.list`: `{project, target?} → {testTargets[{name, kind: unit|ui, cases[]}]}`.
pub type TestListParams = Unspecified;
/// Result of `test.list`.
pub type TestListResult = Unspecified;
crate::rpc_method!(
    /// `test.list` (class `read`). W32 (Wave 4).
    TestList,
    "test.list",
    TestListParams => TestListResult,
    Permission::Read
);

/// Params of `test.run`: `{project, target?, udid?, filter?, ui?: bool, junitPath?} → TestReport`.
pub type TestRunParams = Unspecified;
/// Result of `test.run`.
pub type TestRunResult = Unspecified;
crate::rpc_method!(
    /// `test.run` (class `device`, long-running). W32 (SwiftPM unit tests, Wave 4) / W28 (Xcode projects, XCUITest, Wave 6).
    TestRun,
    "test.run",
    TestRunParams => TestRunResult,
    Permission::Device, long_running
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[TestList::INFO, TestRun::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
