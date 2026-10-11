// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Info.plist synthesis (build §4.4, B06), with the provenance of every key for `project.infoPlist`.
//!
//! Precedence, later wins:
//! 1. product-type defaults, including the UIScene manifest (`UIApplicationSceneManifest`), which iOS 27 requires
//!    of apps linked against the iOS 27 SDK (TN3187; ARCHITECTURE §16.4 item 1) unless the app has its own;
//! 2. `russet.toml [app]`: display name, versions, devices, orientations, launch screen, icon;
//! 3. the project's own Info.plist (`[app] info-plist`), with build settings expanded in its strings
//!    (`INFOPLIST_EXPAND_BUILD_SETTINGS`);
//! 4. partial plists from resource compilers (none until W16/W21 add actool);
//! 5. enforced keys (`CFBundleExecutable`, `CFBundleIdentifier`, `MinimumOSVersion`, `CFBundleSupportedPlatforms`,
//!    `arm64` in `UIRequiredDeviceCapabilities`, the DT* keys of the metadata policy), with a warning when a
//!    project value is replaced.
//!
//! Keys are sorted so the output is stable; the file is written as a binary plist (iOS `INFOPLIST_OUTPUT_FORMAT`).

use std::collections::BTreeMap;

use camino::Utf8Path;
use plist::{Dictionary, Value};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_plist::macros::SettingsStack;
use russet_sdk::Version;

use crate::manifest::{DeviceFamily, LaunchScreen, Orientation};
use crate::metadata;

/// What is being described.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductKind {
    App,
    /// An app extension of `kind` (`widgetkit`, `share`, …).
    AppExtension {
        kind: String,
    },
}

/// The inputs of one Info.plist.
#[derive(Debug, Clone)]
pub struct InfoPlistInput<'a> {
    pub kind: ProductKind,
    /// The product (and executable) name.
    pub product: &'a str,
    pub display_name: Option<&'a str>,
    /// The effective bundle ID.
    pub bundle_id: &'a str,
    pub version: &'a str,
    pub build_number: &'a str,
    pub deployment_target: Version,
    pub devices: &'a [DeviceFamily],
    pub orientations: &'a BTreeMap<DeviceFamily, Vec<Orientation>>,
    pub launch_screen: Option<&'a LaunchScreen>,
    /// `CFBundleIconFile` (the stem of a loose PNG, development builds only).
    pub icon_file: Option<&'a str>,
    /// The project's Info.plist and its label for provenance (a project-relative path).
    pub user_plist: Option<(&'a Utf8Path, &'a str)>,
    /// The DT* keys of the metadata policy.
    pub dt: &'a Dictionary,
    /// Build settings for expanding the project's Info.plist.
    pub settings: &'a SettingsStack,
}

/// A synthesized Info.plist.
#[derive(Debug, Clone, PartialEq)]
pub struct Synthesized {
    pub plist: Dictionary,
    /// Key → where its value came from: `default`, `russet.toml`, the project plist's path, `enforced`, `sdk`.
    pub provenance: BTreeMap<String, String>,
    /// Project values replaced by enforced keys.
    pub overridden: Vec<String>,
}

const DEFAULT: &str = "default";
const MANIFEST: &str = "russet.toml";
const ENFORCED: &str = "enforced";
const SDK: &str = "sdk";

struct Layered {
    plist: Dictionary,
    provenance: BTreeMap<String, String>,
}

impl Layered {
    fn set(&mut self, key: &str, value: Value, source: &str) {
        self.plist.insert(key.to_owned(), value);
        self.provenance.insert(key.to_owned(), source.to_owned());
    }
}

fn strings<'a>(items: impl IntoIterator<Item = &'a str>) -> Value {
    Value::Array(items.into_iter().map(Value::from).collect())
}

/// The NSExtension point of the common extension kinds.
pub fn extension_point(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "widgetkit" | "widget" => "com.apple.widgetkit-extension",
        "share" => "com.apple.share-services",
        "action" => "com.apple.ui-services",
        "notification-service" => "com.apple.usernotifications.service",
        "notification-content" => "com.apple.usernotifications.content-extension",
        "intents" => "com.apple.intents-service",
        "intents-ui" => "com.apple.intents-ui-service",
        "file-provider" => "com.apple.fileprovider-nonui",
        "keyboard" => "com.apple.keyboard-service",
        "safari-web" => "com.apple.Safari.web-extension",
        _ => return None,
    })
}

/// Synthesizes one Info.plist.
pub fn synthesize(input: &InfoPlistInput<'_>) -> Result<Synthesized> {
    let mut l = Layered { plist: Dictionary::new(), provenance: BTreeMap::new() };
    let app = input.kind == ProductKind::App;

    // 1. Product-type defaults.
    l.set("CFBundleDevelopmentRegion", Value::from("en"), DEFAULT);
    l.set("CFBundleInfoDictionaryVersion", Value::from("6.0"), DEFAULT);
    l.set("CFBundleName", Value::from(input.product), DEFAULT);
    l.set("CFBundleDisplayName", Value::from(input.product), DEFAULT);
    l.set("CFBundleShortVersionString", Value::from("1.0.0"), DEFAULT);
    l.set("CFBundleVersion", Value::from("1"), DEFAULT);
    if app {
        l.set("CFBundlePackageType", Value::from("APPL"), DEFAULT);
        l.set("LSRequiresIPhoneOS", Value::Boolean(true), DEFAULT);
        l.set("UIDeviceFamily", Value::Array(vec![Value::from(1)]), DEFAULT);
        let iphone = [
            "UIInterfaceOrientationPortrait",
            "UIInterfaceOrientationLandscapeLeft",
            "UIInterfaceOrientationLandscapeRight",
        ];
        l.set("UISupportedInterfaceOrientations", strings(iphone), DEFAULT);
        l.set("UILaunchScreen", Value::Dictionary(Dictionary::new()), DEFAULT);
        let mut scenes = Dictionary::new();
        scenes.insert("UIApplicationSupportsMultipleScenes".into(), Value::Boolean(true));
        scenes.insert("UISceneConfigurations".into(), Value::Dictionary(Dictionary::new()));
        l.set("UIApplicationSceneManifest", Value::Dictionary(scenes), DEFAULT);
    } else {
        l.set("CFBundlePackageType", Value::from("XPC!"), DEFAULT);
        let mut ext = Dictionary::new();
        if let ProductKind::AppExtension { kind } = &input.kind
            && let Some(point) = extension_point(kind)
        {
            ext.insert("NSExtensionPointIdentifier".into(), Value::from(point));
        }
        l.set("NSExtension", Value::Dictionary(ext), DEFAULT);
    }

    // 2. russet.toml [app].
    if let Some(name) = input.display_name {
        l.set("CFBundleDisplayName", Value::from(name), MANIFEST);
    }
    l.set("CFBundleShortVersionString", Value::from(input.version), MANIFEST);
    l.set("CFBundleVersion", Value::from(input.build_number), MANIFEST);
    if app {
        let families: Vec<Value> = input
            .devices
            .iter()
            .map(|d| {
                Value::from(match d {
                    DeviceFamily::Iphone => 1,
                    DeviceFamily::Ipad => 2,
                })
            })
            .collect();
        if !families.is_empty() {
            l.set("UIDeviceFamily", Value::Array(families), MANIFEST);
        }
        if input.devices.contains(&DeviceFamily::Ipad) {
            let all = [
                "UIInterfaceOrientationPortrait",
                "UIInterfaceOrientationPortraitUpsideDown",
                "UIInterfaceOrientationLandscapeLeft",
                "UIInterfaceOrientationLandscapeRight",
            ];
            l.set("UISupportedInterfaceOrientations~ipad", strings(all), DEFAULT);
        }
        for (family, values) in input.orientations {
            let key = match family {
                DeviceFamily::Iphone => "UISupportedInterfaceOrientations",
                DeviceFamily::Ipad => "UISupportedInterfaceOrientations~ipad",
            };
            l.set(key, strings(values.iter().map(|o| o.plist_value())), MANIFEST);
        }
        if let Some(ls) = input.launch_screen {
            let mut d = Dictionary::new();
            if let Some(c) = &ls.color {
                d.insert("UIColorName".into(), Value::from(c.as_str()));
            }
            if let Some(i) = &ls.image {
                d.insert("UIImageName".into(), Value::from(i.as_str()));
            }
            if let Some(s) = ls.respects_safe_area {
                d.insert("UIImageRespectsSafeAreaInsets".into(), Value::Boolean(s));
            }
            l.set("UILaunchScreen", Value::Dictionary(d), MANIFEST);
        }
        if let Some(icon) = input.icon_file {
            l.set("CFBundleIconFile", Value::from(icon), MANIFEST);
        }
    }

    // 3. The project's own Info.plist, expanded.
    let mut user_keys: BTreeMap<String, Value> = BTreeMap::new();
    if let Some((path, label)) = input.user_plist {
        let user = russet_plist::dictionary_from_path(path).map_err(|e| {
            Error::new(codes::config::INVALID, format!("{label}: {e}"))
                .with_hint("`[app] info-plist` must name a property list whose root is a dictionary.")
        })?;
        for (k, v) in user {
            let v = expand_value(v, input.settings).map_err(|e| {
                Error::new(codes::config::INVALID, format!("{label}: key {k}: {e}")).with_hint(
                    "Build-setting references in Info.plist values use $(NAME) and the documented operators.",
                )
            })?;
            user_keys.insert(k.clone(), v.clone());
            l.set(&k, v, label);
        }
    }

    // 5. Enforced keys.
    let mut overridden = Vec::new();
    let mut enforce = |l: &mut Layered, key: &str, value: Value, source: &str| {
        if let Some(user) = user_keys.get(key)
            && *user != value
        {
            overridden.push(key.to_owned());
        }
        l.set(key, value, source);
    };
    enforce(&mut l, "CFBundleExecutable", Value::from(input.product), ENFORCED);
    enforce(&mut l, "CFBundleIdentifier", Value::from(input.bundle_id), ENFORCED);
    enforce(&mut l, "MinimumOSVersion", Value::from(input.deployment_target.to_string()), ENFORCED);
    enforce(&mut l, "CFBundleSupportedPlatforms", strings(["iPhoneOS"]), ENFORCED);
    if app {
        let caps = match l.plist.get("UIRequiredDeviceCapabilities").cloned() {
            Some(Value::Array(mut items)) => {
                if !items.iter().any(|v| v.as_string() == Some("arm64")) {
                    items.push(Value::from("arm64"));
                }
                Value::Array(items)
            }
            Some(Value::Dictionary(mut d)) => {
                d.insert("arm64".into(), Value::Boolean(true));
                Value::Dictionary(d)
            }
            _ => strings(["arm64"]),
        };
        let source = if user_keys.contains_key("UIRequiredDeviceCapabilities") {
            input.user_plist.map_or(ENFORCED, |(_, label)| label).to_owned()
        } else {
            ENFORCED.to_owned()
        };
        l.set("UIRequiredDeviceCapabilities", caps, &source);
    }
    let stale: Vec<String> = l.plist.keys().filter(|k| metadata::is_metadata_key(k)).cloned().collect();
    for k in stale {
        if !input.dt.contains_key(&k) {
            overridden.push(k.clone());
            l.plist.remove(&k);
            l.provenance.remove(&k);
        }
    }
    for (k, v) in input.dt {
        if user_keys.get(k).is_some_and(|u| u != v) {
            overridden.push(k.clone());
        }
        l.set(k, v.clone(), SDK);
    }

    l.plist.sort_keys();
    overridden.sort();
    overridden.dedup();
    Ok(Synthesized { plist: l.plist, provenance: l.provenance, overridden })
}

/// Expands build-setting references in every string of `v`.
fn expand_value(v: Value, settings: &SettingsStack) -> std::result::Result<Value, russet_plist::macros::MacroError> {
    Ok(match v {
        Value::String(s) if s.contains('$') => Value::String(settings.expand(&s)?),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|i| expand_value(i, settings)).collect::<std::result::Result<_, _>>()?)
        }
        Value::Dictionary(d) => {
            let mut out = Dictionary::new();
            for (k, v) in d {
                out.insert(k, expand_value(v, settings)?);
            }
            Value::Dictionary(out)
        }
        other => other,
    })
}

/// The JSON form of a plist value (`project.infoPlist`, `build.inspect`): dictionaries, arrays, strings, numbers
/// and booleans map directly; dates become RFC 3339 strings and data base64 strings.
pub fn to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Dictionary(d) => serde_json::Value::Object(d.iter().map(|(k, v)| (k.clone(), to_json(v))).collect()),
        Value::Array(a) => serde_json::Value::Array(a.iter().map(to_json).collect()),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Integer(i) => i
            .as_signed()
            .map(serde_json::Value::from)
            .or_else(|| i.as_unsigned().map(serde_json::Value::from))
            .unwrap_or_default(),
        Value::Real(r) => serde_json::Number::from_f64(*r).map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::Date(d) => serde_json::Value::String(d.to_xml_format()),
        Value::Data(bytes) => serde_json::Value::String(russet_plist::base64(bytes)),
        Value::Uid(u) => serde_json::Value::from(u.get()),
        _ => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn dt() -> Dictionary {
        let mut d = Dictionary::new();
        d.insert("DTSDKName".into(), Value::from("iphoneos27.0"));
        d.insert("DTPlatformBuild".into(), Value::from("24A430"));
        d
    }

    fn input<'a>(
        dt: &'a Dictionary,
        settings: &'a SettingsStack,
        orientations: &'a BTreeMap<DeviceFamily, Vec<Orientation>>,
    ) -> InfoPlistInput<'a> {
        InfoPlistInput {
            kind: ProductKind::App,
            product: "RussetProbe",
            display_name: None,
            bundle_id: "dev.russet.test.probe",
            version: "1.0.0",
            build_number: "7",
            deployment_target: Version::new(17, 0, 0),
            devices: &[DeviceFamily::Iphone, DeviceFamily::Ipad],
            orientations,
            launch_screen: None,
            icon_file: None,
            user_plist: None,
            dt,
            settings,
        }
    }

    #[test]
    fn apps_get_the_scene_manifest_and_enforced_keys() {
        let (dt, settings, o) = (dt(), SettingsStack::new(), BTreeMap::new());
        let s = synthesize(&input(&dt, &settings, &o)).unwrap();
        let p = &s.plist;
        let scenes = p.get("UIApplicationSceneManifest").and_then(Value::as_dictionary).unwrap();
        assert_eq!(scenes.get("UIApplicationSupportsMultipleScenes").and_then(Value::as_boolean), Some(true));
        assert!(scenes.get("UISceneConfigurations").and_then(Value::as_dictionary).is_some());
        assert_eq!(p.get("CFBundleIdentifier").and_then(Value::as_string), Some("dev.russet.test.probe"));
        assert_eq!(p.get("CFBundleExecutable").and_then(Value::as_string), Some("RussetProbe"));
        assert_eq!(p.get("MinimumOSVersion").and_then(Value::as_string), Some("17.0"));
        assert_eq!(p.get("CFBundleVersion").and_then(Value::as_string), Some("7"));
        assert_eq!(p.get("UIRequiredDeviceCapabilities"), Some(&strings(["arm64"])));
        assert_eq!(p.get("CFBundleSupportedPlatforms"), Some(&strings(["iPhoneOS"])));
        assert_eq!(p.get("UIDeviceFamily"), Some(&Value::Array(vec![Value::from(1), Value::from(2)])));
        assert!(p.contains_key("UISupportedInterfaceOrientations~ipad"));
        assert_eq!(p.get("DTPlatformBuild").and_then(Value::as_string), Some("24A430"));
        assert_eq!(s.provenance.get("DTSDKName").map(String::as_str), Some("sdk"));
        assert_eq!(s.provenance.get("CFBundleVersion").map(String::as_str), Some("russet.toml"));
        let keys: Vec<&String> = p.keys().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "keys are sorted");
    }

    #[test]
    fn the_project_plist_merges_and_enforced_keys_win() {
        let dir = tempfile::tempdir().unwrap();
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().join("Info.plist")).unwrap();
        let mut user = Dictionary::new();
        user.insert("CFBundleIdentifier".into(), Value::from("wrong.id"));
        user.insert("NSCameraUsageDescription".into(), Value::from("$(PRODUCT_NAME) scans codes"));
        user.insert(
            "UIRequiredDeviceCapabilities".into(),
            Value::Dictionary(Dictionary::from_iter([("metal".to_owned(), Value::Boolean(true))])),
        );
        user.insert("DTXcode".into(), Value::from("10989"));
        user.insert(
            "UIApplicationSceneManifest".into(),
            Value::Dictionary(Dictionary::from_iter([(
                "UIApplicationSupportsMultipleScenes".to_owned(),
                Value::Boolean(false),
            )])),
        );
        russet_plist::to_path(&path, &Value::Dictionary(user), russet_plist::Format::Xml).unwrap();
        let (dt, o) = (dt(), BTreeMap::new());
        let mut settings = SettingsStack::new();
        settings.set("PRODUCT_NAME", "Probe");
        let mut i = input(&dt, &settings, &o);
        i.user_plist = Some((&path, "Resources/Info.plist"));
        let s = synthesize(&i).unwrap();
        let p = &s.plist;
        assert_eq!(p.get("CFBundleIdentifier").and_then(Value::as_string), Some("dev.russet.test.probe"));
        assert_eq!(p.get("NSCameraUsageDescription").and_then(Value::as_string), Some("Probe scans codes"));
        let caps = p.get("UIRequiredDeviceCapabilities").and_then(Value::as_dictionary).unwrap();
        assert_eq!(caps.get("arm64").and_then(Value::as_boolean), Some(true));
        assert_eq!(caps.get("metal").and_then(Value::as_boolean), Some(true));
        assert!(!p.contains_key("DTXcode"));
        let scenes = p.get("UIApplicationSceneManifest").and_then(Value::as_dictionary).unwrap();
        assert_eq!(
            scenes.get("UIApplicationSupportsMultipleScenes").and_then(Value::as_boolean),
            Some(false),
            "the app's own manifest wins"
        );
        assert_eq!(s.provenance.get("NSCameraUsageDescription").map(String::as_str), Some("Resources/Info.plist"));
        assert_eq!(s.overridden, vec!["CFBundleIdentifier".to_owned(), "DTXcode".to_owned()]);
    }

    #[test]
    fn extensions_get_nsextension_and_no_app_keys() {
        let (dt, settings, o) = (dt(), SettingsStack::new(), BTreeMap::new());
        let mut i = input(&dt, &settings, &o);
        i.kind = ProductKind::AppExtension { kind: "widgetkit".into() };
        i.product = "HelloWidget";
        i.bundle_id = "dev.russet.test.probe.widget";
        let p = synthesize(&i).unwrap().plist;
        assert_eq!(p.get("CFBundlePackageType").and_then(Value::as_string), Some("XPC!"));
        let ext = p.get("NSExtension").and_then(Value::as_dictionary).unwrap();
        assert_eq!(
            ext.get("NSExtensionPointIdentifier").and_then(Value::as_string),
            Some("com.apple.widgetkit-extension")
        );
        assert!(!p.contains_key("UIApplicationSceneManifest") && !p.contains_key("LSRequiresIPhoneOS"));
    }

    #[test]
    fn orientations_and_launch_screen_come_from_the_manifest() {
        let (dt, settings) = (dt(), SettingsStack::new());
        let o = BTreeMap::from([(DeviceFamily::Iphone, vec![Orientation::Portrait])]);
        let ls = LaunchScreen { color: Some("Bg".into()), image: None, respects_safe_area: Some(false) };
        let mut i = input(&dt, &settings, &o);
        i.launch_screen = Some(&ls);
        i.display_name = Some("Probe!");
        let p = synthesize(&i).unwrap().plist;
        assert_eq!(p.get("UISupportedInterfaceOrientations"), Some(&strings(["UIInterfaceOrientationPortrait"])));
        let launch = p.get("UILaunchScreen").and_then(Value::as_dictionary).unwrap();
        assert_eq!(launch.get("UIColorName").and_then(Value::as_string), Some("Bg"));
        assert_eq!(p.get("CFBundleDisplayName").and_then(Value::as_string), Some("Probe!"));
    }

    #[test]
    fn json_form_maps_every_type() {
        let mut d = Dictionary::new();
        d.insert("a".into(), Value::from(1));
        d.insert("b".into(), Value::Data(vec![1, 2, 3]));
        d.insert("c".into(), Value::Array(vec![Value::Boolean(true), Value::Real(0.5)]));
        assert_eq!(to_json(&Value::Dictionary(d)), serde_json::json!({"a": 1, "b": "AQID", "c": [true, 0.5]}));
    }
}
