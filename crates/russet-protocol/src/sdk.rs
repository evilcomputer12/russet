// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `sdk.*`: SDK registry and the native SDK/DDI importer (ARCHITECTURE §16.3). Owner: W3.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.1 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `sdk.list`: `{} → {sdks: SdkInfo[]}`.
pub type SdkListParams = Unspecified;
/// Result of `sdk.list`.
pub type SdkListResult = Unspecified;
crate::rpc_method!(
    /// `sdk.list` (class `read`). W3 (Wave 1).
    SdkList,
    "sdk.list",
    SdkListParams => SdkListResult,
    Permission::Read
);

/// Params of `sdk.import`: `{source, parts?: (sdk|ddi)[], want?: ios|full, deleteSourceAfter?, dryRun?} → {sdk?, ddi: {kind: imported, …}|{kind: missing, reason}|{kind: skipped}, bytesWritten, maxAbsPathLen, links{…}}`.
pub type SdkImportParams = Unspecified;
/// Result of `sdk.import`.
pub type SdkImportResult = Unspecified;
crate::rpc_method!(
    /// `sdk.import` (class `system`, long-running). W16b (Wave 2, the DDI part first).
    SdkImport,
    "sdk.import",
    SdkImportParams => SdkImportResult,
    Permission::System, long_running
);

/// Params of `sdk.remove`: `{id} → {freedBytes}`.
pub type SdkRemoveParams = Unspecified;
/// Result of `sdk.remove`.
pub type SdkRemoveResult = Unspecified;
crate::rpc_method!(
    /// `sdk.remove` (class `system`). W16b (Wave 2).
    SdkRemove,
    "sdk.remove",
    SdkRemoveParams => SdkRemoveResult,
    Permission::System
);

/// Params of `sdk.deviceSupport.check`: `{xcodeBuild?} → {kind: none, forXcode}|{kind: available, ddiBuild, url, bytes, authentication}; not v1 (Q25)`.
pub type SdkDeviceSupportCheckParams = Unspecified;
/// Result of `sdk.deviceSupport.check`.
pub type SdkDeviceSupportCheckResult = Unspecified;
crate::rpc_method!(
    /// `sdk.deviceSupport.check` (class `read`). W33 (deferred).
    SdkDeviceSupportCheck,
    "sdk.deviceSupport.check",
    SdkDeviceSupportCheckParams => SdkDeviceSupportCheckResult,
    Permission::Read
);

/// Params of `sdk.deviceSupport.fetch`: `{entry, consent: true} → ImportReport; not v1 (Q25)`.
pub type SdkDeviceSupportFetchParams = Unspecified;
/// Result of `sdk.deviceSupport.fetch`.
pub type SdkDeviceSupportFetchResult = Unspecified;
crate::rpc_method!(
    /// `sdk.deviceSupport.fetch` (class `system`, long-running). W33 (deferred).
    SdkDeviceSupportFetch,
    "sdk.deviceSupport.fetch",
    SdkDeviceSupportFetchParams => SdkDeviceSupportFetchResult,
    Permission::System, long_running
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] =
    &[SdkList::INFO, SdkImport::INFO, SdkRemove::INFO, SdkDeviceSupportCheck::INFO, SdkDeviceSupportFetch::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
