// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The iOS 26/27 signing lints (SIGN §8.6): L-SIG (signature rules, from the verifier), L-MACHO (load commands)
//! and L-BND (bundle layout). Profiles: `device-install` and `app-store`; errors block, warnings do not.

use std::path::Path;

use plist::Value;
use russet_core::Result;
use russet_macho::consts::PLATFORM_IOS;
use russet_macho::{Arch, MachO, Version};

use crate::bundle::{AppBundle, BundleKind};
use crate::util::slash_path;
use crate::verify::{VerifyOptions, verify_app};
use crate::{LintFinding, LintProfile, Severity};

const SDK_27: Version = Version::new(27, 0, 0);
const STORE_MIN_SDK: Version = Version::new(26, 0, 0);
const STORE_MIN_OS: Version = Version::new(13, 0, 0);

pub(crate) fn lint(app: &AppBundle, profile: LintProfile, ipa_root: Option<&Path>) -> Result<Vec<LintFinding>> {
    let store = profile == LintProfile::AppStore;
    let mut out = Vec::new();

    // L-SIG: the verifier's rules, once the app carries a real (CMS) signature. A build product is unsigned or
    // only linker-signed (ad hoc), which the signer replaces.
    let signed = app
        .executable_path()
        .and_then(|p| MachO::open(&p).ok())
        .and_then(|m| m.code_signature())
        .is_some_and(|s| s.has_cms_signature());
    if signed {
        out.extend(verify_app(app, &VerifyOptions::default())?.findings);
    } else {
        out.push(LintFinding::new(
            "L-SIG-000",
            Severity::Info,
            Some(".".into()),
            "the app is not signed yet (no signature, or only an ad-hoc linker signature); the L-SIG rules were skipped",
        ));
    }

    let app_version = app.info_string("CFBundleShortVersionString").map(str::to_owned);
    let app_build = app.info_string("CFBundleVersion").map(str::to_owned);
    let app_id = app.identifier().map(str::to_owned);
    let app_min_os = app.info_string("MinimumOSVersion").and_then(|s| s.parse::<Version>().ok());

    for b in app.all() {
        let rel_of = |p: &Path| {
            let r = slash_path(p);
            if b.rel_path.is_empty() { r } else { format!("{}/{r}", b.rel_path) }
        };
        let at = |p: &str| Some(if p.is_empty() { ".".to_owned() } else { p.to_owned() });
        let min_os = b.info_string("MinimumOSVersion").and_then(|s| s.parse::<Version>().ok()).or(app_min_os);

        // L-MACHO on every Mach-O of the bundle.
        for item in &b.code {
            let rel = rel_of(item.path());
            let Ok(m) = MachO::open(&b.root.join(item.path())) else { continue };
            if m.is_fat() || m.slices().iter().any(|s| s.arch != Arch::Arm64) {
                let arches: Vec<String> = m.slices().iter().map(|s| s.arch.name()).collect();
                out.push(LintFinding::new(
                    "L-MACHO-004",
                    Severity::Error,
                    Some(rel.clone()),
                    format!("expected a thin arm64 Mach-O, found {}", arches.join(", ")),
                ));
            }
            for img in m.images() {
                let Some(bv) = &img.build_version else {
                    if store {
                        out.push(LintFinding::new(
                            "L-MACHO-002",
                            Severity::Error,
                            Some(rel.clone()),
                            "no LC_BUILD_VERSION (uploads need platform iOS and sdk ≥ 26.0)",
                        ));
                    }
                    continue;
                };
                if img.is_64 && bv.sdk >= SDK_27 && img.string_pool_alignment() < 8 {
                    out.push(LintFinding::new(
                        "L-MACHO-001",
                        Severity::Error,
                        Some(rel.clone()),
                        format!(
                            "the LC_SYMTAB string pool is {}-byte aligned; iOS 27's dyld needs 8 (re-sign to fix)",
                            img.string_pool_alignment()
                        ),
                    ));
                }
                if bv.platform != PLATFORM_IOS {
                    out.push(LintFinding::new(
                        "L-MACHO-002",
                        Severity::Error,
                        Some(rel.clone()),
                        format!("LC_BUILD_VERSION platform is {}, not iOS (2)", bv.platform),
                    ));
                } else if store && bv.sdk < STORE_MIN_SDK {
                    out.push(LintFinding::new(
                        "L-MACHO-002",
                        Severity::Error,
                        Some(rel.clone()),
                        format!("linked against the iOS {} SDK; uploads need 26.0 or later (ITMS-90725)", bv.sdk),
                    ));
                }
                if let Some(min) = min_os
                    && bv.minos > min
                {
                    out.push(LintFinding::new(
                        "L-MACHO-003",
                        Severity::Error,
                        Some(rel.clone()),
                        format!("minos {} is above the bundle's MinimumOSVersion {min}", bv.minos),
                    ));
                }
            }
        }
        if store
            && b.rel_path.is_empty()
            && let Some(min) = min_os
            && min < STORE_MIN_OS
        {
            out.push(LintFinding::new(
                "L-MACHO-003",
                Severity::Error,
                at(""),
                format!("MinimumOSVersion {min} is below 13.0, the App Store floor"),
            ));
        }

        // L-BND-005: unresolved build variables in any Info.plist.
        if let Some(key) = unresolved_variable(&b.info) {
            out.push(LintFinding::new(
                "L-BND-005",
                Severity::Error,
                at(&format!("{}{}Info.plist", b.rel_path, if b.rel_path.is_empty() { "" } else { "/" })),
                format!("Info.plist key {key} still contains an unresolved $(…) variable"),
            ));
        }

        match b.kind {
            BundleKind::AppExtension | BundleKind::AppClip | BundleKind::WatchApp => {
                // L-BND-004: extension IDs are prefixed by the app's ID.
                if let (Some(app_id), Some(id)) = (&app_id, b.identifier())
                    && !id.starts_with(&format!("{app_id}."))
                {
                    out.push(LintFinding::new(
                        "L-BND-004",
                        Severity::Error,
                        at(&b.rel_path),
                        format!("the bundle ID {id} is not prefixed by the app's {app_id}."),
                    ));
                }
                if store && b.kind == BundleKind::AppExtension {
                    if b.root.join("Frameworks").is_dir() {
                        out.push(LintFinding::new(
                            "L-BND-001",
                            Severity::Error,
                            at(&b.rel_path),
                            "app extensions may not contain a Frameworks/ directory (ITMS-90206)",
                        ));
                    }
                    let same = b.info_string("CFBundleShortVersionString").map(str::to_owned) == app_version
                        && b.info_string("CFBundleVersion").map(str::to_owned) == app_build;
                    if !same {
                        out.push(LintFinding::new(
                            "L-BND-003",
                            Severity::Error,
                            at(&b.rel_path),
                            "the extension's version and build number must equal the app's (ITMS-90473)",
                        ));
                    }
                    if let Some(caps) = b.info.get("UIRequiredDeviceCapabilities")
                        && !requires_arm64(caps)
                    {
                        out.push(LintFinding::new(
                            "L-BND-003",
                            Severity::Error,
                            at(&b.rel_path),
                            "UIRequiredDeviceCapabilities must include arm64 (ITMS-90502)",
                        ));
                    }
                }
            }
            BundleKind::App if b.rel_path.is_empty() => {
                lint_app_bundle(b, store, ipa_root, &mut out);
            }
            _ => {}
        }
    }
    Ok(out)
}

fn lint_app_bundle(b: &AppBundle, store: bool, ipa_root: Option<&Path>, out: &mut Vec<LintFinding>) {
    let at = Some(".".to_owned());
    // L-BND-002 and L-BND-010: loose dylibs.
    let frameworks = b.root.join("Frameworks");
    let mut swift_dylibs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&frameworks) {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".dylib"))
            .collect();
        names.sort();
        for n in names {
            if n.starts_with("libswift") {
                swift_dylibs.push(n);
            } else if store {
                out.push(LintFinding::new(
                    "L-BND-002",
                    Severity::Error,
                    Some(format!("Frameworks/{n}")),
                    "loose dylibs are rejected by the App Store; wrap it in a .framework",
                ));
            }
        }
    }
    if store && !swift_dylibs.is_empty() {
        match ipa_root {
            Some(root) => {
                for n in &swift_dylibs {
                    if !root.join("SwiftSupport/iphoneos").join(n).is_file() {
                        out.push(LintFinding::new(
                            "L-BND-010",
                            Severity::Error,
                            Some(format!("Frameworks/{n}")),
                            "the IPA lacks the matching SwiftSupport/iphoneos/ copy (Invalid Swift Support)",
                        ));
                    }
                }
            }
            None => out.push(LintFinding::new(
                "L-BND-010",
                Severity::Info,
                at.clone(),
                "Swift runtime dylibs are embedded; check SwiftSupport/ on the store IPA (`russet sign lint App.ipa`)",
            )),
        }
    }
    // L-BND-006: UIDeviceFamily and the DT* group.
    if !b.info.contains_key("UIDeviceFamily") || !b.info.contains_key("DTPlatformName") {
        out.push(LintFinding::new(
            "L-BND-006",
            if store { Severity::Error } else { Severity::Warning },
            at.clone(),
            "Info.plist lacks UIDeviceFamily or the DT* keys; iOS 26+ installs but may not register the app",
        ));
    }
    // L-BND-007: an app icon for the store.
    if store {
        let has_icon_name = b.info.contains_key("CFBundleIconName") || b.info.contains_key("CFBundleIcons");
        if !has_icon_name || !b.root.join("Assets.car").is_file() {
            out.push(LintFinding::new(
                "L-BND-007",
                Severity::Error,
                at.clone(),
                "the App Store needs CFBundleIconName (or CFBundleIcons) and an Assets.car with the app icon (ITMS-90713/90717)",
            ));
        }
    }
    // L-BND-009: iOS 27 SDK apps declare a scene manifest (TN3187).
    let sdk27 = b
        .executable_path()
        .and_then(|p| MachO::open(&p).ok())
        .and_then(|m| m.build_version())
        .is_some_and(|bv| bv.sdk >= SDK_27);
    if sdk27 && !b.info.contains_key("UIApplicationSceneManifest") {
        out.push(LintFinding::new(
            "L-BND-009",
            Severity::Warning,
            at,
            "linked with the iOS 27 SDK but Info.plist has no UIApplicationSceneManifest (TN3187)",
        ));
    }
}

fn requires_arm64(caps: &Value) -> bool {
    match caps {
        Value::Array(a) => a.iter().any(|v| v.as_string() == Some("arm64")),
        Value::Dictionary(d) => d.get("arm64").and_then(Value::as_boolean) == Some(true),
        _ => false,
    }
}

/// The first key (dotted path) whose string value still contains `$(` or `${`.
fn unresolved_variable(d: &plist::Dictionary) -> Option<String> {
    fn walk(v: &Value, path: &str) -> Option<String> {
        match v {
            Value::String(s) if s.contains("$(") || s.contains("${") => Some(path.to_owned()),
            Value::Array(a) => a.iter().enumerate().find_map(|(i, e)| walk(e, &format!("{path}[{i}]"))),
            Value::Dictionary(d) => {
                d.iter().find_map(|(k, e)| walk(e, &if path.is_empty() { k.clone() } else { format!("{path}.{k}") }))
            }
            _ => None,
        }
    }
    d.iter().find_map(|(k, v)| walk(v, k))
}
