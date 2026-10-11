// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `sign.*`: code signing, verification and lints (ARCHITECTURE §8.4; SIGN §8). Owner: W6.
//!
//! Results are objects (ARCHITECTURE §7.3): `sign.lint` answers `{findings}` and `sign.inspect` `{signatures}`.
//! Team IDs are masked (`*******XYZ`) and no certificate subject is reported (it names a person).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod};

/// The distribution a bundle is signed for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename = "SignDistribution")]
#[ts(rename = "SignDistribution")]
pub enum Distribution {
    #[default]
    Development,
    AdHoc,
    AppStoreConnect,
}

/// How profile-only entitlements are treated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename = "SignMergePolicy")]
#[ts(rename = "SignMergePolicy")]
pub enum MergePolicy {
    /// Least privilege (the default).
    #[default]
    Least,
    /// Copy the profile's keys (re-signing third-party apps).
    ProfileAll,
}

/// A lint profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename = "SignLintProfile")]
#[ts(rename = "SignLintProfile")]
pub enum LintProfile {
    DeviceInstall,
    AppStore,
}

/// A finding's severity: errors block signing and uploads; warnings do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename = "SignSeverity")]
#[ts(rename = "SignSeverity")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// One lint finding (L-SIG, L-MACHO, L-BND; SIGN §8.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "SignLintFinding")]
#[ts(rename = "SignLintFinding")]
pub struct LintFinding {
    /// For example `L-SIG-001`.
    pub code: String,
    pub severity: Severity,
    pub message: String,
    /// The file or bundle, relative to the checked bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
}

/// Params of `sign.run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignRunParams {
    /// The `.app` to sign (absolute).
    pub path: String,
    pub distribution: Distribution,
    /// `<team>/<serial>`, as `identity.list` shows it; else the newest identity the profiles list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub identity_id: Option<String>,
    /// Profiles by bundle path (`""` = the app); others come from the profile cache (`provision.apply`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, String>,
    /// Requested entitlements (`.entitlements` files) by bundle path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entitlements: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub merge_policy: Option<MergePolicy>,
    /// Sign a copy here (an existing previous output is replaced); omitted: sign in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub output: Option<String>,
    /// `get-task-allow` for every app-like bundle (debug builds); omitted: each profile's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub get_task_allow: Option<bool>,
    /// A device the cached profiles must list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device: Option<String>,
    /// Pre-sign fixes to skip: `chmod`, `align-string-pool`, `clean`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_fix: Vec<String>,
    /// The lints that must pass after signing (default `device-install`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub lint: Option<LintProfile>,
    /// Skip the post-sign lints (verification still runs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub no_lint: Option<bool>,
}

/// One signed app-like bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignedBundle {
    /// `""` for the app.
    pub rel_path: String,
    pub bundle_id: String,
    /// `app`, `app-extension`, `app-clip`, `watch-app`.
    pub kind: String,
    /// The CDHash (20 bytes, hex) the install ledger records.
    pub cdhash: String,
    /// Masked.
    pub team_id: String,
    pub profile_uuid: String,
    /// Merge notes (for example `aps-environment` taken from the profile).
    #[serde(default)]
    pub notes: Vec<String>,
}

/// One pre-sign fix that changed a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "SignFixRecord")]
#[ts(rename = "SignFixRecord")]
pub struct FixRecord {
    /// `chmod`, `align-string-pool`, `clean`, `thin`.
    pub fix: String,
    pub path: String,
}

/// Result of `sign.run` (`SignReport`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignRunResult {
    /// The signed bundle.
    pub output: String,
    pub bundles: Vec<SignedBundle>,
    #[serde(default)]
    pub fixes_applied: Vec<FixRecord>,
    #[serde(default)]
    pub lints: Vec<LintFinding>,
}
crate::rpc_method!(
    /// `sign.run` (class `build`, long-running). W6 (Wave 1).
    SignRun,
    "sign.run",
    SignRunParams => SignRunResult,
    Permission::Build, long_running
);

/// Params of `sign.verify`, `sign.lint` and `sign.inspect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignPathParams {
    /// An `.app` directory or `.ipa` (and, for `sign.inspect`, any Mach-O file).
    pub path: String,
    /// `sign.verify`: also run these lints; `sign.lint`: the profile (default `device-install`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile: Option<LintProfile>,
    /// `sign.verify`: a device every embedded development profile must list (L-SIG-007).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device: Option<String>,
}

/// One verified bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedBundle {
    pub rel_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cdhash: Option<String>,
    /// Masked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub team_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile_uuid: Option<String>,
    pub signed: bool,
}

/// Params of `sign.verify`.
pub type SignVerifyParams = SignPathParams;
/// Result of `sign.verify` (`VerifyReport`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignVerifyResult {
    /// No finding of severity error.
    pub ok: bool,
    pub path: String,
    pub bundles: Vec<VerifiedBundle>,
    pub findings: Vec<LintFinding>,
}
crate::rpc_method!(
    /// `sign.verify` (class `read`). W6 (Wave 1).
    SignVerify,
    "sign.verify",
    SignVerifyParams => SignVerifyResult,
    Permission::Read
);

/// Params of `sign.lint`.
pub type SignLintParams = SignPathParams;
/// Result of `sign.lint`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignLintResult {
    pub findings: Vec<LintFinding>,
}
crate::rpc_method!(
    /// `sign.lint` (class `read`). W6 (Wave 1).
    SignLint,
    "sign.lint",
    SignLintParams => SignLintResult,
    Permission::Read
);

/// One code directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CodeDirectoryInfo {
    /// For example `0x20400`.
    pub version: String,
    /// `sha1`, `sha256`, …
    pub hash_type: String,
    pub page_size: u32,
    /// `adhoc`, `linker-signed`, `runtime`, …
    pub flags: Vec<String>,
    /// `main-binary`, `allow-unsigned`, …
    pub exec_seg_flags: Vec<String>,
    pub cdhash: String,
}

/// One Mach-O slice's signature (`SignatureInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInfo {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
    pub arch: String,
    /// `execute`, `dylib`, …
    pub filetype: String,
    pub signed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub identifier: Option<String>,
    /// Masked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub code_directories: Vec<CodeDirectoryInfo>,
    /// Entitlement keys only (values can embed the team ID).
    #[serde(default)]
    pub entitlement_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub get_task_allow: Option<bool>,
    pub has_der_entitlements: bool,
    pub has_cms: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub platform: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub minos: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sdk: Option<String>,
    pub string_pool_aligned: bool,
}

/// Params of `sign.inspect`.
pub type SignInspectParams = SignPathParams;
/// Result of `sign.inspect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignInspectResult {
    pub signatures: Vec<SignatureInfo>,
}
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn wire_shapes() {
        let p: SignRunParams = serde_json::from_value(serde_json::json!({
            "path": "/a/App.app", "distribution": "app-store-connect", "profiles": {"": "/p.mobileprovision"},
            "mergePolicy": "profile-all", "noFix": ["chmod"], "unknownField": 1
        }))
        .unwrap();
        assert_eq!(p.distribution, Distribution::AppStoreConnect);
        assert_eq!(p.merge_policy, Some(MergePolicy::ProfileAll));
        assert_eq!(p.profiles.get("").map(String::as_str), Some("/p.mobileprovision"));
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("identityId").is_none(), "optional fields are omitted");
        let f = LintFinding { code: "L-SIG-001".into(), severity: Severity::Error, message: "m".into(), path: None };
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            serde_json::json!({"code": "L-SIG-001", "severity": "error", "message": "m"})
        );
        let q: SignPathParams =
            serde_json::from_value(serde_json::json!({"path": "/x", "profile": "device-install"})).unwrap();
        assert_eq!(q.profile, Some(LintProfile::DeviceInstall));
    }
}
