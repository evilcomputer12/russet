// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `sdk.*`: SDK registry and the native SDK/DDI importer (ARCHITECTURE §16.3). Owner: W3 (`sdk.list`); W16b
//! (`sdk.import`, `sdk.remove`); W33 (`sdk.deviceSupport.*`, deferred).
//!
//! The import, remove and Device Support DTOs are still placeholders ([`Unspecified`]); their owners replace
//! each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize, JsonSchema, TS` (camelCase; the
//! wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use crate::toolchain::SdkInfo;
use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `sdk.list`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SdkListParams {}

/// Result of `sdk.list`: Russet's imports first, then xtool bundles read in place.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SdkListResult {
    pub sdks: Vec<SdkInfo>,
}

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
