// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `doctor.*`: environment checks (ARCHITECTURE §8.1; core §11). Owner: W3 (area checks register through
//! russetd's doctor seam).
//!
//! `doctor.run` is `read`, or `system` with `fix` (resolved per call; ceiling `system`) [D 66, D 98]. The report
//! is the JSON of core §11.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod};

/// A check's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorStatus {
    Pass,
    Warn,
    Fail,
    Skip,
}

/// Report grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorCategory {
    Host,
    Daemon,
    Config,
    Secrets,
    Usbmux,
    Device,
    Toolchain,
    Account,
    Network,
    Platform,
}

/// How a failed or warning check can be fixed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DoctorFix {
    /// `doctor.run {fix: true}` can fix it without root; `applied` says whether this run did.
    Auto {
        #[serde(default)]
        applied: bool,
    },
    /// A command for the user to run (for example one that needs root).
    Command { command: String },
    /// The user has to act; the hint says how.
    Manual,
}

/// Params of `doctor.run`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DoctorRunParams {
    /// Run only these check ids (`toolchain.linker`), or id prefixes ending in `.` (`toolchain.`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub checks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub categories: Option<Vec<DoctorCategory>>,
    /// The project root, for the project checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    /// The device, for the device checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// Also run the checks that need the network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub online: Option<bool>,
    /// Apply the automatic fixes (class `system`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fix: Option<bool>,
}

/// The Russet build that ran the checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DoctorBuild {
    pub version: String,
    pub build_id: String,
}

/// The host the checks ran on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DoctorHost {
    pub os: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub distro: Option<String>,
    pub arch: String,
    pub wsl: bool,
}

/// One check's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    /// Stable id, for example `toolchain.linker`.
    pub id: String,
    pub category: DoctorCategory,
    pub title: String,
    pub status: DoctorStatus,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fix: Option<DoctorFix>,
    #[ts(type = "number")]
    pub duration_ms: u64,
}

/// Counts per status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DoctorSummary {
    pub pass: u32,
    pub warn: u32,
    pub fail: u32,
    pub skip: u32,
}

/// Result of `doctor.run` (core §11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub russet: DoctorBuild,
    pub host: DoctorHost,
    pub checks: Vec<DoctorCheck>,
    pub summary: DoctorSummary,
}

/// Result of `doctor.run`.
pub type DoctorRunResult = DoctorReport;

crate::rpc_method!(
    /// `doctor.run` (class `read`, `fix` → `system`; resolved per call, ceiling `system`). MCP lists `doctor_run`
    /// at tier 0 without `fix` (ARCHITECTURE §11.4). W3 (+ area checks) (Wave 1).
    DoctorRun,
    "doctor.run",
    DoctorRunParams => DoctorRunResult,
    Permission::System, permission_fn = doctor_run_permission
);

/// Per-call class of `doctor.run` [D 66, D 98]: `read`, or `system` with `fix`.
pub fn doctor_run_permission(
    p: &DoctorRunParams,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(if p.fix == Some(true) { Permission::System } else { Permission::Read })
}

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[DoctorRun::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
