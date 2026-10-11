// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `project.*`: project detection and info (ARCHITECTURE §8.3). Owner: W5 (Wave 1), W16 (project.init).
//!
//! `project.init` keeps its F0 placeholder DTOs until W16 (Wave 2) defines them.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// A project's stack (`[project] stack`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectStack {
    Swiftpm,
    Flutter,
    Xcode,
    ReactNative,
    Maui,
}

/// What a target builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum TargetKind {
    App,
    AppExtension,
}

/// Params of `project.detect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetectParams {
    /// Any directory inside the project; the project root is found upwards from it.
    pub dir: String,
}

/// Result of `project.detect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetectResult {
    pub stack: ProjectStack,
    /// The absolute project root.
    pub root: String,
    /// Products the project declares (SwiftPM library products, read without running SwiftPM).
    pub products: Vec<String>,
    /// `russet.toml`, `russet.local.toml` and the stack's manifest files, absolute.
    pub manifests: Vec<String>,
}
crate::rpc_method!(
    /// `project.detect` (class `read`). W5 (Wave 1).
    ProjectDetect,
    "project.detect",
    ProjectDetectParams => ProjectDetectResult,
    Permission::Read
);

/// Params of `project.info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfoParams {
    /// The absolute project root (or a directory inside it).
    pub project: String,
}

/// One target of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTarget {
    pub name: String,
    /// The canonical bundle ID from `russet.toml`; omitted until `[app] bundle-id` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
    pub kind: TargetKind,
}

/// Result of `project.info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfoResult {
    pub stack: ProjectStack,
    pub root: String,
    /// `[project] name`, else the app product, else the directory name.
    pub name: String,
    pub targets: Vec<ProjectTarget>,
    /// `debug`, `release`, `profile` and every `[build.profiles.<name>]`.
    pub configurations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_target: Option<String>,
}
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

/// Params of `project.infoPlist`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfoPlistParams {
    pub project: String,
    /// The app product (default) or an extension product.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<String>,
    /// `debug` (default), `release`, `profile` or a named profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub configuration: Option<String>,
}

/// Result of `project.infoPlist`: the Info.plist a build would write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfoPlistResult {
    /// The plist's JSON form: dictionaries, arrays, strings, numbers and booleans as JSON; dates as RFC 3339
    /// strings; data as base64 strings.
    pub plist: serde_json::Value,
    /// Key → its source: `default`, `russet.toml`, the project's Info.plist path, `enforced` or `sdk`.
    pub provenance: BTreeMap<String, String>,
}
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
