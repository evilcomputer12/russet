// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The DT* metadata policy (ARCHITECTURE §16.4) [D 17] and the normalization of nested bundles.
//!
//! The values come only from the SDK the build uses: the platform `Info.plist` `AdditionalInfo` template
//! (`DTSDKName = $(SDK_NAME)`, …) expanded with build settings derived from [`SdkFacts`], never constants.
//!
//! | Key | `none` | `minimal` | `sdk` (default) | `xcode-compat` (opt-in + consent) |
//! |---|---|---|---|---|
//! | `DTPlatformName`, `DTPlatformVersion`, `DTSDKName` | – | ✓ | ✓ | ✓ |
//! | `DTSDKBuild`, `DTPlatformBuild` | – | – | ✓ | ✓ |
//! | `DTCompiler`, `DTXcode`, `DTXcodeBuild` | – | – | – | ✓ |
//! | `BuildMachineOSBuild` | – | – | – | only with an explicit `MAC_OS_X_PRODUCT_BUILD_VERSION` override |
//! | `RussetToolchain` `{tool, version, host}` | – | – | – | ✓ (visible disclosure) |
//!
//! Every Info.plist of a bundle Russet built (SwiftPM resource bundles, extensions, wrapped frameworks) gets the
//! app's DT* keys, which replaces Swift Build's Linux stamps (`DTXcode = 10989`, `DTXcodeBuild = 99T999`, an
//! empty `DTPlatformBuild`) [V build V9].

use std::collections::BTreeMap;

use plist::{Dictionary, Value};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_plist::macros::SettingsStack;
use russet_sdk::{SdkFacts, Version};

use crate::MetadataPolicy;

/// Keys each policy writes, in addition to the previous level's.
const MINIMAL: &[&str] = &["DTPlatformName", "DTPlatformVersion", "DTSDKName"];
const SDK: &[&str] = &["DTSDKBuild", "DTPlatformBuild"];
const XCODE: &[&str] = &["DTCompiler", "DTXcode", "DTXcodeBuild"];

/// Whether `key` is one of the keys the metadata policy owns.
pub fn is_metadata_key(key: &str) -> bool {
    key.starts_with("DT") || key == "BuildMachineOSBuild" || key == "RussetToolchain"
}

/// The build settings the SDK supplies (`SDK_NAME`, `PLATFORM_PRODUCT_BUILD_VERSION`, …) for a deployment target.
pub fn sdk_settings(facts: &SdkFacts, deployment_target: Version) -> BTreeMap<String, String> {
    let mut s = BTreeMap::new();
    let mut set = |k: &str, v: &str| {
        s.insert(k.to_owned(), v.to_owned());
    };
    set("PLATFORM_NAME", "iphoneos");
    set("SDK_NAME", &facts.sdk_canonical_name);
    set("SDK_VERSION", &facts.sdk_version.to_string());
    set("SDK_PRODUCT_BUILD_VERSION", facts.sdk_build.as_deref().unwrap_or_default());
    set("PLATFORM_PRODUCT_BUILD_VERSION", facts.platform_build.as_deref().unwrap_or_default());
    set("XCODE_VERSION_ACTUAL", &facts.xcode_version.xcode_actual());
    set("XCODE_PRODUCT_BUILD_VERSION", &facts.xcode_build);
    if let Some(c) = &facts.default_compiler {
        set("DEFAULT_COMPILER", c);
        set("GCC_VERSION", "$(DEFAULT_COMPILER)");
    }
    set("DEPLOYMENT_TARGET_SETTING_NAME", "IPHONEOS_DEPLOYMENT_TARGET");
    set("IPHONEOS_DEPLOYMENT_TARGET", &deployment_target.to_string());
    s
}

/// The DT* keys `policy` writes, expanded from the SDK's template.
pub fn dt_keys(policy: &MetadataPolicy, facts: &SdkFacts, settings: &SettingsStack) -> Result<Dictionary> {
    let mut wanted: Vec<&str> = Vec::new();
    match policy {
        MetadataPolicy::None => {}
        MetadataPolicy::Minimal => wanted.extend(MINIMAL),
        MetadataPolicy::Sdk => wanted.extend(MINIMAL.iter().chain(SDK)),
        MetadataPolicy::XcodeCompat { consent } => {
            if consent.by.trim().is_empty() || consent.at.trim().is_empty() {
                return Err(Error::new(codes::config::INVALID, "metadata = \"xcode-compat\" needs a recorded consent")
                    .with_hint("Commit `[build] metadata-consent = { by = \"<name>\", at = \"<date>\" }` in russet.toml (ARCHITECTURE §16.4)."));
            }
            wanted.extend(MINIMAL.iter().chain(SDK).chain(XCODE));
        }
    }
    let mut out = Dictionary::new();
    for key in &wanted {
        let Some(template) = facts.additional_info.get(key).and_then(Value::as_string) else {
            continue;
        };
        let value = settings.expand(template).map_err(|e| {
            Error::new(codes::sdk::INCOMPLETE, format!("cannot expand {key} = {template}: {e}"))
                .with_hint(
                    "The SDK's platform Info.plist has a template Russet cannot expand: use a complete SDK, or set \
                     `[build] metadata = \"minimal\"` (ARCHITECTURE §16.4).",
                )
                .with_details(serde_json::json!({"key": key, "template": template, "bundle": facts.bundle}))
        })?;
        if value.is_empty() {
            if SDK.contains(key) {
                let missing = if *key == "DTSDKBuild" {
                    "the SDK's System/Library/CoreServices/SystemVersion.plist"
                } else {
                    "the iPhoneOS platform version.plist"
                };
                return Err(Error::new(
                    codes::sdk::INCOMPLETE,
                    format!("{key} needs {missing}, which the SDK bundle {} lacks", facts.bundle),
                )
                .with_hint("Use a complete SDK, or set `[build] metadata = \"minimal\"` (ARCHITECTURE §16.4).")
                .with_details(serde_json::json!({"key": key, "missing": missing})));
            }
            // Swift Build elides empty Xcode-identity keys; so does Russet.
            continue;
        }
        out.insert((*key).to_owned(), Value::String(value));
    }
    if matches!(policy, MetadataPolicy::XcodeCompat { .. }) {
        if let Some(v) = settings.raw("MAC_OS_X_PRODUCT_BUILD_VERSION").filter(|v| !v.is_empty()) {
            out.insert("BuildMachineOSBuild".into(), Value::String(v.to_owned()));
        }
        let mut marker = Dictionary::new();
        marker.insert("tool".into(), Value::from("russet"));
        marker.insert("version".into(), Value::from(env!("CARGO_PKG_VERSION")));
        marker.insert("host".into(), Value::from(std::env::consts::OS));
        out.insert("RussetToolchain".into(), Value::Dictionary(marker));
    }
    Ok(out)
}

/// Replaces every metadata key of `info` with `dt` (the app's values; empty under `none`). Returns whether
/// anything changed.
pub fn normalize(info: &mut Dictionary, dt: &Dictionary) -> bool {
    let before = info.clone();
    let stale: Vec<String> = info.keys().filter(|k| is_metadata_key(k)).cloned().collect();
    for k in stale {
        info.remove(&k);
    }
    for (k, v) in dt {
        info.insert(k.clone(), v.clone());
    }
    info.sort_keys();
    *info != before
}

/// The DT* values as strings (for `build.inspect` and the build report).
pub fn as_strings(dt: &Dictionary) -> BTreeMap<String, String> {
    dt.iter().filter_map(|(k, v)| Some((k.clone(), v.as_string()?.to_owned()))).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::Consent;

    pub(crate) fn facts() -> SdkFacts {
        let mut info = Dictionary::new();
        for (k, v) in [
            ("BuildMachineOSBuild", "$(MAC_OS_X_PRODUCT_BUILD_VERSION)"),
            ("DTCompiler", "$(GCC_VERSION)"),
            ("DTPlatformBuild", "$(PLATFORM_PRODUCT_BUILD_VERSION)"),
            ("DTPlatformName", "iphoneos"),
            ("DTPlatformVersion", "27.0"),
            ("DTSDKBuild", "$(SDK_PRODUCT_BUILD_VERSION)"),
            ("DTSDKName", "$(SDK_NAME)"),
            ("DTXcode", "$(XCODE_VERSION_ACTUAL)"),
            ("DTXcodeBuild", "$(XCODE_PRODUCT_BUILD_VERSION)"),
            ("MinimumOSVersion", "$($(DEPLOYMENT_TARGET_SETTING_NAME))"),
        ] {
            info.insert(k.into(), Value::from(v));
        }
        SdkFacts {
            id: "xcode-27.0-27A266a".into(),
            sdk_path: "/b/iPhoneOS27.0.sdk".into(),
            sdk_version: Version::new(27, 0, 0),
            sdk_canonical_name: "iphoneos27.0".into(),
            sdk_build: Some("24A430".into()),
            platform_build: Some("24A430".into()),
            platform_version: "27.0".into(),
            xcode_version: Version::new(27, 0, 0),
            xcode_build: "27A266a".into(),
            swift_required: Version::new(6, 4, 0),
            valid_deployment_targets: vec![Version::new(17, 0, 0), Version::new(27, 0, 0)],
            additional_info: info,
            back_deploy_dirs: Vec::new(),
            platforms_view: "/d/view/Developer/Platforms".into(),
            toolset_swb: "/d/toolset-swb.json".into(),
            toolset_native: "/d/toolset-native.json".into(),
            tools: russet_sdk::ToolPaths::default(),
            bundle: "/b".into(),
            default_compiler: Some("com.apple.compilers.llvm.clang.1_0".into()),
            back_deploy_not_needed_from: BTreeMap::new(),
            darwin_tools: Some("1.1.0".into()),
        }
    }

    fn settings(f: &SdkFacts) -> SettingsStack {
        SettingsStack::from(sdk_settings(f, Version::new(17, 0, 0)))
    }

    fn keys(d: &Dictionary) -> Vec<&str> {
        d.keys().map(String::as_str).collect()
    }

    #[test]
    fn each_policy_writes_its_keys() {
        let f = facts();
        let s = settings(&f);
        assert!(dt_keys(&MetadataPolicy::None, &f, &s).unwrap().is_empty());
        assert_eq!(
            keys(&dt_keys(&MetadataPolicy::Minimal, &f, &s).unwrap()),
            ["DTPlatformName", "DTPlatformVersion", "DTSDKName"]
        );
        let sdk = dt_keys(&MetadataPolicy::Sdk, &f, &s).unwrap();
        assert_eq!(
            as_strings(&sdk),
            BTreeMap::from([
                ("DTPlatformBuild".to_owned(), "24A430".to_owned()),
                ("DTPlatformName".to_owned(), "iphoneos".to_owned()),
                ("DTPlatformVersion".to_owned(), "27.0".to_owned()),
                ("DTSDKBuild".to_owned(), "24A430".to_owned()),
                ("DTSDKName".to_owned(), "iphoneos27.0".to_owned()),
            ])
        );
        assert!(!sdk.contains_key("DTXcode") && !sdk.contains_key("DTCompiler"));
    }

    #[test]
    fn xcode_compat_needs_consent_and_adds_the_marker() {
        let f = facts();
        let mut s = settings(&f);
        let none = MetadataPolicy::XcodeCompat { consent: Consent { by: String::new(), at: String::new() } };
        assert_eq!(dt_keys(&none, &f, &s).unwrap_err().code(), codes::config::INVALID);
        let given = MetadataPolicy::XcodeCompat { consent: Consent { by: "owner".into(), at: "2026-10-11".into() } };
        let d = dt_keys(&given, &f, &s).unwrap();
        assert_eq!(d.get("DTXcode").and_then(Value::as_string), Some("2700"));
        assert_eq!(d.get("DTXcodeBuild").and_then(Value::as_string), Some("27A266a"));
        assert_eq!(d.get("DTCompiler").and_then(Value::as_string), Some("com.apple.compilers.llvm.clang.1_0"));
        assert!(!d.contains_key("BuildMachineOSBuild"));
        assert!(d.get("RussetToolchain").and_then(Value::as_dictionary).is_some());
        s.set("MAC_OS_X_PRODUCT_BUILD_VERSION", "26A434");
        let d = dt_keys(&given, &f, &s).unwrap();
        assert_eq!(d.get("BuildMachineOSBuild").and_then(Value::as_string), Some("26A434"));
    }

    #[test]
    fn a_missing_platform_build_is_named() {
        let mut f = facts();
        f.platform_build = None;
        let s = settings(&f);
        let e = dt_keys(&MetadataPolicy::Sdk, &f, &s).unwrap_err();
        assert_eq!(e.code(), codes::sdk::INCOMPLETE);
        assert!(e.message().contains("version.plist"));
        assert!(dt_keys(&MetadataPolicy::Minimal, &f, &s).is_ok());
    }

    #[test]
    fn nested_bundles_lose_swift_builds_linux_stamps() {
        let f = facts();
        let dt = dt_keys(&MetadataPolicy::Sdk, &f, &settings(&f)).unwrap();
        let mut info = Dictionary::new();
        info.insert("CFBundleIdentifier".into(), Value::from("x.resources"));
        info.insert("DTXcode".into(), Value::from("10989"));
        info.insert("DTXcodeBuild".into(), Value::from("99T999"));
        info.insert("DTPlatformBuild".into(), Value::from(""));
        assert!(normalize(&mut info, &dt));
        assert!(!info.contains_key("DTXcode") && !info.contains_key("DTXcodeBuild"));
        assert_eq!(info.get("DTPlatformBuild").and_then(Value::as_string), Some("24A430"));
        assert_eq!(info.get("CFBundleIdentifier").and_then(Value::as_string), Some("x.resources"));
        assert!(!normalize(&mut info, &dt), "a second pass changes nothing");
        assert!(normalize(&mut info, &Dictionary::new()));
        assert!(info.keys().all(|k| !is_metadata_key(k)));
    }
}
