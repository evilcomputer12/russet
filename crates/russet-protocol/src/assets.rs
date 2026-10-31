// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `assets.*`: asset catalogs (ARCHITECTURE §8.3). Owner: W21.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.3 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `assets.list`: `{catalog} → {assets[{name, kind, variants[]}]}`.
pub type AssetsListParams = Unspecified;
/// Result of `assets.list`.
pub type AssetsListResult = Unspecified;
crate::rpc_method!(
    /// `assets.list` (class `read`). W21 (Wave 3).
    AssetsList,
    "assets.list",
    AssetsListParams => AssetsListResult,
    Permission::Read
);

/// Params of `assets.compile`: `{catalog, outDir, appIcon?, accentColor?, deploymentTarget} → {outputs[], partialInfoPlist, diagnostics[]}`.
pub type AssetsCompileParams = Unspecified;
/// Result of `assets.compile`.
pub type AssetsCompileResult = Unspecified;
crate::rpc_method!(
    /// `assets.compile` (class `build`). W21 (Wave 3).
    AssetsCompile,
    "assets.compile",
    AssetsCompileParams => AssetsCompileResult,
    Permission::Build
);

/// Params of `assets.appIcon`: `{source1024, catalog, name?} → {iconSet}`.
pub type AssetsAppIconParams = Unspecified;
/// Result of `assets.appIcon`.
pub type AssetsAppIconResult = Unspecified;
crate::rpc_method!(
    /// `assets.appIcon` (class `project`). W21 (Wave 3).
    AssetsAppIcon,
    "assets.appIcon",
    AssetsAppIconParams => AssetsAppIconResult,
    Permission::Project
);

/// Params of `assets.render`: `{catalog, name, scale?, appearance?} → {pngBase64}`.
pub type AssetsRenderParams = Unspecified;
/// Result of `assets.render`.
pub type AssetsRenderResult = Unspecified;
crate::rpc_method!(
    /// `assets.render` (class `read`). W21 (Wave 3).
    AssetsRender,
    "assets.render",
    AssetsRenderParams => AssetsRenderResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[AssetsList::INFO, AssetsCompile::INFO, AssetsAppIcon::INFO, AssetsRender::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
