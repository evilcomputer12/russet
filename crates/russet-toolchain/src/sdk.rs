// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The SDK registry probe (ARCHITECTURE §14.8, §16.1; core §10.6): Russet's own imports listed in
//! `data/sdks/registry.json` (written by russet-sdk, W16b) and xtool's bundles in `~/.swiftpm/swift-sdks/`, which
//! are read in place and never copied.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::SdkInfo;
use crate::pins::PinManifest;

/// One entry of `data/sdks/registry.json` (camelCase, ARCHITECTURE §14.8).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryEntry {
    id: String,
    bundle: PathBuf,
    xcode_version: String,
    xcode_build: String,
    ios_sdk: String,
    #[serde(default)]
    darwin_tools: Option<String>,
    #[serde(default)]
    swift_sdk_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RegistryFile {
    List(Vec<RegistryEntry>),
    Wrapped { sdks: Vec<RegistryEntry> },
}

/// The SDKs Russet imported (`data/sdks/registry.json`); a missing or unreadable file lists none.
pub fn registry_sdks(data_dir: &Path, pins: &PinManifest) -> Vec<SdkInfo> {
    let Ok(text) = std::fs::read_to_string(data_dir.join("sdks/registry.json")) else { return Vec::new() };
    let entries = match serde_json::from_str::<RegistryFile>(&text) {
        Ok(RegistryFile::List(v)) | Ok(RegistryFile::Wrapped { sdks: v }) => v,
        Err(_) => return Vec::new(),
    };
    entries
        .into_iter()
        .map(|e| SdkInfo {
            compatible_swift: compatible_swift(pins, &e.xcode_version, &e.xcode_build),
            swift_sdk_name: e.swift_sdk_name.unwrap_or_else(|| "darwin".into()),
            id: e.id,
            bundle: e.bundle,
            xcode_version: e.xcode_version,
            xcode_build: e.xcode_build,
            ios_sdk: e.ios_sdk,
            darwin_tools: e.darwin_tools,
        })
        .collect()
}

fn compatible_swift(pins: &PinManifest, version: &str, build: &str) -> String {
    pins.xcode(build).or_else(|| pins.xcode(version)).map(|r| r.swift.clone()).unwrap_or_default()
}

#[derive(Debug, Deserialize)]
struct BundleInfo {
    artifacts: std::collections::BTreeMap<String, BundleArtifact>,
}

#[derive(Debug, Deserialize)]
struct BundleArtifact {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    variants: Vec<BundleVariant>,
}

#[derive(Debug, Deserialize)]
struct BundleVariant {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwiftSdkJson {
    target_triples: std::collections::BTreeMap<String, TripleInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TripleInfo {
    sdk_root_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SdkSettings {
    version: String,
}

/// Reads one xtool-style `*.artifactbundle` in place; `None` when it is not a Darwin SDK bundle.
pub fn read_bundle(bundle: &Path, pins: &PinManifest) -> Option<SdkInfo> {
    let info: BundleInfo = serde_json::from_str(&std::fs::read_to_string(bundle.join("info.json")).ok()?).ok()?;
    let (name, artifact) = info.artifacts.iter().find(|(_, a)| a.kind == "swiftSDK")?;
    let variant = artifact.variants.first().map_or(".", |v| v.path.as_str());
    let root = bundle.join(variant);
    let sdk_json: SwiftSdkJson =
        serde_json::from_str(&std::fs::read_to_string(root.join("swift-sdk.json")).ok()?).ok()?;
    let ios = sdk_json.target_triples.get("arm64-apple-ios")?;
    let settings: SdkSettings =
        serde_json::from_str(&std::fs::read_to_string(root.join(&ios.sdk_root_path).join("SDKSettings.json")).ok()?)
            .ok()?;
    let version_plist = root.join("Xcode.app/Contents/version.plist");
    let plist = plist::Value::from_file(&version_plist).ok()?;
    let dict = plist.as_dictionary()?;
    let xcode_version = dict.get("CFBundleShortVersionString")?.as_string()?.to_owned();
    let xcode_build = dict.get("ProductBuildVersion")?.as_string()?.to_owned();
    let darwin_tools = std::fs::read_to_string(root.join("darwin-sdk-version.txt"))
        .ok()
        .and_then(|t| t.trim().split(',').find_map(|kv| kv.strip_prefix("darwinTools=")).map(str::to_owned));
    Some(SdkInfo {
        id: format!("xcode-{xcode_version}-{xcode_build}"),
        compatible_swift: compatible_swift(pins, &xcode_version, &xcode_build),
        xcode_version,
        xcode_build,
        ios_sdk: settings.version,
        swift_sdk_name: name.clone(),
        bundle: bundle.to_path_buf(),
        darwin_tools,
    })
}

/// xtool's bundles in the SwiftPM SDK directories.
pub fn bundle_sdks(sdk_dirs: &[PathBuf], pins: &PinManifest) -> Vec<SdkInfo> {
    let mut out = Vec::new();
    for dir in sdk_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut bundles: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "artifactbundle"))
            .collect();
        bundles.sort();
        out.extend(bundles.iter().filter_map(|b| read_bundle(b, pins)));
    }
    out
}

/// The linker a bundle's toolset names (`toolset.json`: `rootPath` + `linker.path`).
pub fn bundle_linker(bundle: &Path) -> Option<PathBuf> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Toolset {
        root_path: Option<String>,
        linker: Option<ToolPath>,
    }
    #[derive(Deserialize)]
    struct ToolPath {
        path: String,
    }
    let t: Toolset = serde_json::from_str(&std::fs::read_to_string(bundle.join("toolset.json")).ok()?).ok()?;
    let linker = t.linker?.path;
    let root = t.root_path.map_or_else(|| bundle.to_path_buf(), |r| bundle.join(r));
    Some(if Path::new(&linker).is_absolute() { PathBuf::from(linker) } else { root.join(linker) })
}
