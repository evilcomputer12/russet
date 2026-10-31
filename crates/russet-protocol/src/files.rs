// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `files.*`: device files (media, app containers, crash reports). Owner: W10.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.2 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `files.list`: `{udid?, scope: {kind: media|app-container|app-documents|crash-reports, bundleId?}, path, local?}`.
pub type FilesListParams = Unspecified;
/// Result of `files.list`.
pub type FilesListResult = Unspecified;
crate::rpc_method!(
    /// `files.list` (class `read`). W10 (Wave 2).
    FilesList,
    "files.list",
    FilesListParams => FilesListResult,
    Permission::Read
);

/// Params of `files.pull`: `{udid?, scope, path, local?}`.
pub type FilesPullParams = Unspecified;
/// Result of `files.pull`.
pub type FilesPullResult = Unspecified;
crate::rpc_method!(
    /// `files.pull` (class `read`, long-running). W10 (Wave 2).
    FilesPull,
    "files.pull",
    FilesPullParams => FilesPullResult,
    Permission::Read, long_running
);

/// Params of `files.push`: `{udid?, scope, path, local?}`.
pub type FilesPushParams = Unspecified;
/// Result of `files.push`.
pub type FilesPushResult = Unspecified;
crate::rpc_method!(
    /// `files.push` (class `device`, long-running). W10 (Wave 2).
    FilesPush,
    "files.push",
    FilesPushParams => FilesPushResult,
    Permission::Device, long_running
);

/// Params of `files.remove`: `{udid?, scope, path}`.
pub type FilesRemoveParams = Unspecified;
/// Result of `files.remove`.
pub type FilesRemoveResult = Unspecified;
crate::rpc_method!(
    /// `files.remove` (class `device`). W10 (Wave 2).
    FilesRemove,
    "files.remove",
    FilesRemoveParams => FilesRemoveResult,
    Permission::Device
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[FilesList::INFO, FilesPull::INFO, FilesPush::INFO, FilesRemove::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
