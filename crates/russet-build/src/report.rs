// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `build-report.json`, schema 1 (build §9.6; ARCHITECTURE §14.8): what was built, with which toolchain and SDK,
//! which DT* keys were stamped, every Mach-O's build version, the lints, the artifacts and the signing plan.
//! `buildId` is the job id (ARCHITECTURE §20.1). Written next to the app, at
//! `<project>/.russet/build/<profile>-iphoneos/build-report.json`.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use serde::Serialize;

use crate::bundle::MachOEntry;
use crate::stack::swiftpm::io;
use crate::{LintFinding, SigningPlan};

/// `build-report.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildReport {
    pub schema: u32,
    pub build_id: String,
    pub russet_version: String,
    pub project: ReportProject,
    pub profile: String,
    pub started_at: String,
    pub duration_ms: u64,
    /// Growth of `<project>/.russet` during the build.
    pub disk_delta_bytes: i64,
    pub toolchain: ReportToolchain,
    pub metadata: ReportMetadata,
    pub bundles: Vec<ReportBundle>,
    pub lints: Vec<LintFinding>,
    pub artifacts: ReportArtifacts,
    pub signing_plan: Vec<ReportSignItem>,
}

/// `project`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportProject {
    pub dir: Utf8PathBuf,
    pub stack: String,
    pub product: String,
    pub backend: String,
}

/// `toolchain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportToolchain {
    pub swift: String,
    pub linker: String,
    pub sdk: ReportSdk,
}

/// `toolchain.sdk`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSdk {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    pub xcode: String,
    pub id: String,
}

/// `metadata`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportMetadata {
    pub policy: String,
    pub stamped: BTreeMap<String, String>,
}

/// One bundle of the app.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportBundle {
    pub path: String,
    pub kind: String,
    pub bundle_id: String,
    pub original_bundle_id: String,
    pub mach_o: Vec<ReportMachO>,
}

/// One Mach-O file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportMachO {
    pub path: String,
    pub arch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minos: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub built_by_russet: bool,
}

impl ReportMachO {
    /// `m` as listed under the bundle at `bundle` (both relative to the app's parent directory): the path is
    /// relative to that bundle, as in build §9.6 (`"path": "Hello"` under `Hello.app`).
    pub fn in_bundle(m: &MachOEntry, bundle: &str) -> Self {
        let path = m.path.strip_prefix(bundle).and_then(|p| p.strip_prefix('/')).unwrap_or(&m.path);
        Self {
            path: path.to_owned(),
            arch: m.summary.archs.join(","),
            minos: m.summary.minos.map(|v| v.to_string()),
            sdk: m.summary.sdk.map(|v| v.to_string()),
            tool: m.summary.tools.first().cloned(),
            built_by_russet: m.built_by_russet,
        }
    }
}

/// `artifacts`: paths relative to the project root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportArtifacts {
    pub app: String,
    pub dsym: Option<String>,
    pub ipa: Option<String>,
}

/// One signing-plan item, relative to the project root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSignItem {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entitlements: Option<String>,
    pub get_task_allow: bool,
}

/// `path` relative to `root` when it is inside it.
pub fn rel(root: &Utf8Path, path: &Utf8Path) -> String {
    path.strip_prefix(root).map_or_else(|_| path.to_string(), |p| p.to_string())
}

/// The signing plan as the report lists it.
pub fn signing_items(root: &Utf8Path, plan: &SigningPlan) -> Vec<ReportSignItem> {
    plan.items
        .iter()
        .map(|i| ReportSignItem {
            path: rel(root, &i.path),
            entitlements: i.entitlements.as_ref().map(|e| rel(root, e)),
            get_task_allow: i.get_task_allow,
        })
        .collect()
}

impl BuildReport {
    /// Writes the report as pretty JSON with a trailing newline.
    pub fn write(&self, path: &Utf8Path) -> Result<()> {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        std::fs::write(path, text).map_err(|e| io(path, e))
    }
}

/// The total size of the files under `dir` (0 when it does not exist).
pub fn dir_size(dir: &Utf8Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = d.read_dir_utf8() else { continue };
        for e in entries.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path().to_owned()),
                Ok(t) if t.is_file() => total += e.metadata().map_or(0, |m| m.len()),
                _ => {}
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macho::MachOSummary;

    #[test]
    fn mach_o_paths_are_relative_to_their_bundle() {
        let entry =
            |path: &str| MachOEntry { path: path.into(), summary: MachOSummary::default(), built_by_russet: true };
        assert_eq!(ReportMachO::in_bundle(&entry("Hello.app/Hello"), "Hello.app").path, "Hello");
        assert_eq!(
            ReportMachO::in_bundle(&entry("Hello.app/Frameworks/libswiftX.dylib"), "Hello.app").path,
            "Frameworks/libswiftX.dylib"
        );
        assert_eq!(
            ReportMachO::in_bundle(&entry("Hello.app/PlugIns/W.appex/W"), "Hello.app/PlugIns/W.appex").path,
            "W"
        );
        assert_eq!(ReportMachO::in_bundle(&entry("Other.app/X"), "Hello.app").path, "Other.app/X");
    }
}
