// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Reading an SDK bundle in place (ARCHITECTURE §16.3; build §3.4). The layout is the Swift SDK bundle that xtool
//! writes, which Russet's own importer keeps (W16b):
//!
//! ```text
//! <bundle>/Developer/Platforms/iPhoneOS.platform/{Info.plist, version.plist?, Developer/SDKs/iPhoneOS27.0.sdk}
//! <bundle>/Developer/Toolchains/XcodeDefault.xctoolchain/…
//! <bundle>/Xcode.app/Contents/{version.plist, Developer/Platforms/iPhoneOS.platform/version.plist,
//!                              Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-*/iphoneos}
//! <bundle>/toolset/bin/{ld64.lld, llvm-lib, dsymutil}            (xtool's darwin-tools)
//! <bundle>/darwin-sdk-version.txt                               (`epoch=3,darwinTools=1.1.0,oam=1.3.0`)
//! ```
//!
//! Nothing is copied or modified. A file the build plane cannot work without is `sdk.incomplete` (2103), naming
//! the file.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use plist::{Dictionary, Value};
use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::Version;

/// What a bundle says about itself, before Russet materializes its view and toolsets.
#[derive(Debug, Clone, PartialEq)]
pub struct BundleFacts {
    pub bundle: Utf8PathBuf,
    /// `<bundle>/Developer/Platforms/iPhoneOS.platform`.
    pub platform_dir: Utf8PathBuf,
    /// `<bundle>/Developer/Toolchains`.
    pub toolchains_dir: Utf8PathBuf,
    pub sdk_path: Utf8PathBuf,
    pub sdk_version: Version,
    pub sdk_canonical_name: String,
    pub sdk_build: Option<String>,
    /// The platform `version.plist`, wherever the bundle keeps it.
    pub platform_version_plist: Option<Utf8PathBuf>,
    pub platform_build: Option<String>,
    pub platform_version: String,
    pub xcode_version: Version,
    pub xcode_build: String,
    pub swift_required: Version,
    pub valid_deployment_targets: Vec<Version>,
    pub additional_info: Dictionary,
    pub back_deploy_dirs: Vec<Utf8PathBuf>,
    pub default_compiler: Option<String>,
    pub back_deploy_not_needed_from: BTreeMap<String, Version>,
    pub darwin_tools: Option<String>,
    /// `<bundle>/toolset/bin` when the bundle ships its own linker (xtool's darwin-tools).
    pub linker_dir: Option<Utf8PathBuf>,
}

impl BundleFacts {
    /// `xcode-<Xcode version>-<Xcode build>`, as `toolchain.list` reports SDKs (ARCHITECTURE §5.3 `SdkInfo`).
    pub fn id(&self) -> String {
        format!("xcode-{}-{}", self.xcode_version, self.xcode_build)
    }

    /// Reads `bundle` in place.
    pub fn read(bundle: &Utf8Path) -> Result<Self> {
        let platform_dir = bundle.join("Developer/Platforms/iPhoneOS.platform");
        if !platform_dir.is_dir() {
            return Err(incomplete(bundle, "Developer/Platforms/iPhoneOS.platform"));
        }
        let sdk_path = find_sdk(bundle, &platform_dir)?;
        let settings = read_dict(bundle, &sdk_path.join("SDKSettings.plist"))
            .or_else(|_| read_dict(bundle, &sdk_path.join("SDKSettings.json")))?;
        let sdk_version = string(&settings, "Version")
            .and_then(Version::parse)
            .ok_or_else(|| incomplete_key(bundle, &sdk_path.join("SDKSettings.plist"), "Version"))?;
        let sdk_canonical_name = string(&settings, "CanonicalName")
            .map(str::to_owned)
            .ok_or_else(|| incomplete_key(bundle, &sdk_path.join("SDKSettings.plist"), "CanonicalName"))?;
        let target = settings
            .get("SupportedTargets")
            .and_then(Value::as_dictionary)
            .and_then(|t| t.get("iphoneos"))
            .and_then(Value::as_dictionary);
        let valid_deployment_targets: Vec<Version> = target
            .and_then(|t| t.get("ValidDeploymentTargets"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_string).filter_map(Version::parse).collect())
            .unwrap_or_default();
        let back_deploy_not_needed_from = target
            .and_then(|t| t.get("SwiftCompatibilityLibraryNotNeededStartingIn"))
            .and_then(Value::as_dictionary)
            .map(|d| d.iter().filter_map(|(k, v)| Some((k.clone(), v.as_string().and_then(Version::parse)?))).collect())
            .unwrap_or_default();
        let default_compiler = settings
            .get("DefaultProperties")
            .and_then(Value::as_dictionary)
            .and_then(|d| string(d, "DEFAULT_COMPILER"))
            .map(str::to_owned);

        let system_version = sdk_path.join("System/Library/CoreServices/SystemVersion.plist");
        let sdk_build =
            read_dict(bundle, &system_version).ok().and_then(|d| string(&d, "ProductBuildVersion").map(str::to_owned));

        let platform_info = read_dict(bundle, &platform_dir.join("Info.plist"))?;
        let additional_info = platform_info
            .get("AdditionalInfo")
            .and_then(Value::as_dictionary)
            .cloned()
            .ok_or_else(|| incomplete_key(bundle, &platform_dir.join("Info.plist"), "AdditionalInfo"))?;
        let platform_version = string(&platform_info, "Version")
            .or_else(|| string(&additional_info, "DTPlatformVersion"))
            .map_or_else(|| sdk_version.to_string(), str::to_owned);

        let xcode_contents = [bundle.join("Xcode.app/Contents"), bundle.join("Contents")];
        let platform_version_plist = std::iter::once(platform_dir.join("version.plist"))
            .chain(xcode_contents.iter().map(|c| c.join("Developer/Platforms/iPhoneOS.platform/version.plist")))
            .find(|p| p.is_file());
        let platform_build = platform_version_plist
            .as_ref()
            .and_then(|p| read_dict(bundle, p).ok())
            .and_then(|d| string(&d, "ProductBuildVersion").map(str::to_owned));

        let xcode_version_plist = xcode_contents
            .iter()
            .map(|c| c.join("version.plist"))
            .find(|p| p.is_file())
            .ok_or_else(|| incomplete(bundle, "Xcode.app/Contents/version.plist"))?;
        let xcode = read_dict(bundle, &xcode_version_plist)?;
        let xcode_version = string(&xcode, "CFBundleShortVersionString")
            .and_then(Version::parse)
            .ok_or_else(|| incomplete_key(bundle, &xcode_version_plist, "CFBundleShortVersionString"))?;
        let xcode_build = string(&xcode, "ProductBuildVersion")
            .map(str::to_owned)
            .ok_or_else(|| incomplete_key(bundle, &xcode_version_plist, "ProductBuildVersion"))?;

        let swift_required =
            swift_from_interfaces(&sdk_path).or_else(|| swift_for_xcode(xcode_version)).ok_or_else(|| {
                Error::new(
                    codes::sdk::INCOMPATIBLE,
                    format!("cannot tell which Swift version the Xcode {xcode_version} SDK in {bundle} needs"),
                )
                .with_hint("Russet knows the SDKs of Xcode 26 and 27; import one of those (`russet sdk import`).")
            })?;

        let mut back_deploy_dirs = Vec::new();
        for contents in xcode_contents.iter().map(|c| c.join("Developer")).chain([bundle.join("Developer")]) {
            let lib = contents.join("Toolchains/XcodeDefault.xctoolchain/usr/lib");
            let Ok(entries) = lib.read_dir_utf8() else { continue };
            let mut dirs: Vec<Utf8PathBuf> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().starts_with("swift-"))
                .map(|e| e.path().join("iphoneos"))
                .filter(|p| p.is_dir())
                .collect();
            dirs.sort();
            for d in dirs {
                if !back_deploy_dirs.contains(&d) {
                    back_deploy_dirs.push(d);
                }
            }
        }

        let darwin_tools = std::fs::read_to_string(bundle.join("darwin-sdk-version.txt")).ok().and_then(|s| {
            s.trim().split(',').find_map(|kv| kv.strip_prefix("darwinTools=").map(|v| v.trim().to_owned()))
        });
        let linker_dir = Some(bundle.join("toolset/bin")).filter(|d| d.join("ld64.lld").is_file());

        Ok(Self {
            bundle: bundle.to_owned(),
            toolchains_dir: bundle.join("Developer/Toolchains"),
            platform_dir,
            sdk_path,
            sdk_version,
            sdk_canonical_name,
            sdk_build,
            platform_version_plist,
            platform_build,
            platform_version,
            xcode_version,
            xcode_build,
            swift_required,
            valid_deployment_targets,
            additional_info,
            back_deploy_dirs,
            default_compiler,
            back_deploy_not_needed_from,
            darwin_tools,
            linker_dir,
        })
    }
}

/// `iPhoneOS<version>.sdk` when present (Xcode's canonical name), else `iPhoneOS.sdk`.
fn find_sdk(bundle: &Utf8Path, platform_dir: &Utf8Path) -> Result<Utf8PathBuf> {
    let sdks = platform_dir.join("Developer/SDKs");
    let entries =
        sdks.read_dir_utf8().map_err(|_| incomplete(bundle, "Developer/Platforms/iPhoneOS.platform/Developer/SDKs"))?;
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_owned())
        .filter(|n| n.starts_with("iPhoneOS") && n.ends_with(".sdk"))
        .collect();
    names.sort();
    let versioned = names.iter().rev().find(|n| n.len() > "iPhoneOS.sdk".len());
    let name = versioned
        .or_else(|| names.iter().find(|n| *n == "iPhoneOS.sdk"))
        .ok_or_else(|| incomplete(bundle, "Developer/Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS*.sdk"))?;
    Ok(sdks.join(name))
}

/// The Swift version the SDK's own `Swift.swiftmodule` interfaces were built with
/// (`// swift-compiler-version: Apple Swift version 6.4 …`).
fn swift_from_interfaces(sdk: &Utf8Path) -> Option<Version> {
    let dir = sdk.join("usr/lib/swift/Swift.swiftmodule");
    let mut files: Vec<Utf8PathBuf> = dir
        .read_dir_utf8()
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path().to_owned())
        .filter(|p| p.extension() == Some("swiftinterface"))
        .collect();
    files.sort();
    files.iter().find_map(|f| {
        let text = std::fs::read_to_string(f).ok()?;
        text.lines().take(5).find_map(|l| {
            let rest = l.strip_prefix("// swift-compiler-version:")?;
            let after = rest.split("Swift version").nth(1)?;
            Version::parse(after.split_whitespace().next()?)
        })
    })
}

/// Fallback when the interfaces are missing: the Swift version each supported Xcode ships (§14.7 `[[xcode]]`).
fn swift_for_xcode(xcode: Version) -> Option<Version> {
    match (xcode.major, xcode.minor) {
        (27, _) => Some(Version::new(6, 4, 0)),
        (26, m) if m >= 4 => Some(Version::new(6, 3, 0)),
        (26, _) => Some(Version::new(6, 2, 0)),
        _ => None,
    }
}

fn string<'a>(d: &'a Dictionary, key: &str) -> Option<&'a str> {
    d.get(key).and_then(Value::as_string)
}

/// The hint of an SDK file that exists but cannot be read.
const DAMAGED_HINT: &str =
    "The SDK bundle is damaged: recreate it (`xtool setup` for xtool's bundle, or `russet sdk import` again).";

fn read_dict(bundle: &Utf8Path, path: &Utf8Path) -> Result<Dictionary> {
    if path.extension() == Some("json") {
        let text = std::fs::read_to_string(path).map_err(|_| incomplete_path(bundle, path))?;
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            Error::new(codes::sdk::INCOMPLETE, format!("{path} is not valid JSON: {e}")).with_hint(DAMAGED_HINT)
        })?;
        return match json_to_plist(json) {
            Value::Dictionary(d) => Ok(d),
            _ => Err(Error::new(codes::sdk::INCOMPLETE, format!("{path} is not a dictionary")).with_hint(DAMAGED_HINT)),
        };
    }
    if !path.is_file() {
        return Err(incomplete_path(bundle, path));
    }
    russet_plist::dictionary_from_path(path).map_err(|e| {
        Error::new(codes::sdk::INCOMPLETE, format!("{path} cannot be read: {e}")).with_hint(DAMAGED_HINT).caused_by(e)
    })
}

fn json_to_plist(v: serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::String(String::new()),
        serde_json::Value::Bool(b) => Value::Boolean(b),
        serde_json::Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => Value::from(i),
            (None, Some(u), _) => Value::from(u),
            (_, _, f) => Value::Real(f.unwrap_or_default()),
        },
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(a) => Value::Array(a.into_iter().map(json_to_plist).collect()),
        serde_json::Value::Object(o) => Value::Dictionary(o.into_iter().map(|(k, v)| (k, json_to_plist(v))).collect()),
    }
}

fn relative<'a>(bundle: &Utf8Path, path: &'a Utf8Path) -> &'a Utf8Path {
    path.strip_prefix(bundle).unwrap_or(path)
}

fn incomplete_path(bundle: &Utf8Path, path: &Utf8Path) -> Error {
    incomplete(bundle, relative(bundle, path).as_str())
}

fn incomplete_key(bundle: &Utf8Path, path: &Utf8Path, key: &str) -> Error {
    let rel = relative(bundle, path);
    Error::new(codes::sdk::INCOMPLETE, format!("the SDK bundle {bundle} has no `{key}` in {rel}"))
        .with_hint(INCOMPLETE_HINT)
        .with_details(serde_json::json!({"bundle": bundle, "missing": format!("{rel}:{key}")}))
}

const INCOMPLETE_HINT: &str = "Use a complete SDK: xtool's normal bundle (with its Xcode.app copy) or a Russet import \
     (`russet sdk import <Xcode_27.xip>`, ARCHITECTURE §16.3).";

pub(crate) fn incomplete(bundle: &Utf8Path, missing: &str) -> Error {
    Error::new(codes::sdk::INCOMPLETE, format!("the SDK bundle {bundle} lacks {missing}"))
        .with_hint(INCOMPLETE_HINT)
        .with_details(serde_json::json!({"bundle": bundle, "missing": missing}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swift_fallback_table() {
        assert_eq!(swift_for_xcode(Version::new(27, 0, 0)), Some(Version::new(6, 4, 0)));
        assert_eq!(swift_for_xcode(Version::new(26, 4, 0)), Some(Version::new(6, 3, 0)));
        assert_eq!(swift_for_xcode(Version::new(26, 0, 0)), Some(Version::new(6, 2, 0)));
        assert_eq!(swift_for_xcode(Version::new(15, 0, 0)), None);
    }

    #[test]
    fn json_settings_convert_to_plist_values() {
        let v = json_to_plist(serde_json::json!({"Version": "27.0", "N": 3, "F": 1.5, "B": true, "L": ["a"]}));
        let d = v.as_dictionary().cloned().unwrap_or_default();
        assert_eq!(d.get("Version").and_then(Value::as_string), Some("27.0"));
        assert_eq!(d.get("N").and_then(Value::as_signed_integer), Some(3));
        assert_eq!(d.get("F").and_then(Value::as_real), Some(1.5));
        assert_eq!(d.get("B").and_then(Value::as_boolean), Some(true));
    }
}
