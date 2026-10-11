// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `project.*`: project detection and info (ARCHITECTURE §8.3). Owner: W5 (Wave 1), W16 (project.init).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.3 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `project.detect`: `{dir} → {stack, root, products[], manifests[]}`.
pub type ProjectDetectParams = Unspecified;
/// Result of `project.detect`.
pub type ProjectDetectResult = Unspecified;
crate::rpc_method!(
    /// `project.detect` (class `read`). W5 (Wave 1).
    ProjectDetect,
    "project.detect",
    ProjectDetectParams => ProjectDetectResult,
    Permission::Read
);

/// Params of `project.info`: `{project} → {stack, root, targets[{name, bundleId, kind}], configurations[], defaultTarget}`.
pub type ProjectInfoParams = Unspecified;
/// Result of `project.info`.
pub type ProjectInfoResult = Unspecified;
crate::rpc_method!(
    /// `project.info` (class `read`). W5 (Wave 1).
    ProjectInfo,
    "project.info",
    ProjectInfoParams => ProjectInfoResult,
    Permission::Read
);

/// Params of `project.init`: `{dir, stack, name, bundleId} → {files[]}`.
pub type ProjectInitParams = Unspecified;
/// Result of `project.init`.
pub type ProjectInitResult = Unspecified;
crate::rpc_method!(
    /// `project.init` (class `project`). W16 (Wave 2).
    ProjectInit,
    "project.init",
    ProjectInitParams => ProjectInitResult,
    Permission::Project
);

/// Params of `project.infoPlist`: `{project, target?, configuration?} → {plist, provenance{key: source}}`.
pub type ProjectInfoPlistParams = Unspecified;
/// Result of `project.infoPlist`.
pub type ProjectInfoPlistResult = Unspecified;
crate::rpc_method!(
    /// `project.infoPlist` (class `read`). W5 (Wave 1).
    ProjectInfoPlist,
    "project.infoPlist",
    ProjectInfoPlistParams => ProjectInfoPlistResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[ProjectDetect::INFO, ProjectInfo::INFO, ProjectInit::INFO, ProjectInfoPlist::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
