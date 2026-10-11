// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The build plane's view of `russet.toml` and `russet.local.toml` (ARCHITECTURE §14.2) [D 15, D 16].
//!
//! The sections the build plane owns (`[project]`, `[app]`, `[build]`) are typed and strict: an unknown key is
//! `config.unknownKey` and a bad value `config.invalid`, both with `file:line:col` (build B02). Of `[signing]`
//! only `bundle-id-policy` and `team` are read here; the other sections belong to other areas and pass through.
//! `russet.local.toml` (per user, gitignored) is layered over `russet.toml` key by key (§14.1).
//!
//! Field names and defaults match W3's `russet_config::project` model of the same file, so this loader can be
//! replaced by `russet_config` once W3 merges, without touching its callers.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_core::error::{Error, codes};
use serde::Deserialize;

/// The committed project file.
pub const PROJECT_FILE: &str = "russet.toml";
/// The per-user override file (§14.1).
pub const LOCAL_FILE: &str = "russet.local.toml";

/// `russet.toml` with `russet.local.toml` layered over it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct ProjectManifest {
    pub schema: Option<u32>,
    pub project: ProjectSection,
    pub app: AppSection,
    pub build: BuildSection,
    pub signing: SigningSection,
    // Sections other areas own; W3's russet-config validates them.
    pub run: Option<toml::Table>,
    pub debug: Option<toml::Table>,
    pub ship: Option<toml::Table>,
    pub toolchain: Option<toml::Table>,
    pub agents: Option<toml::Table>,
}

/// `[project]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct ProjectSection {
    pub name: Option<String>,
    pub stack: Option<Stack>,
}

/// A project stack (`[project] stack`; auto-detected when omitted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stack {
    Swiftpm,
    Flutter,
    Xcode,
    ReactNative,
    Maui,
}

impl Stack {
    /// The wire and file name.
    pub fn as_str(self) -> &'static str {
        match self {
            Stack::Swiftpm => "swiftpm",
            Stack::Flutter => "flutter",
            Stack::Xcode => "xcode",
            Stack::ReactNative => "react-native",
            Stack::Maui => "maui",
        }
    }
}

/// `[app]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct AppSection {
    pub bundle_id: Option<String>,
    pub display_name: Option<String>,
    pub version: Option<String>,
    /// `CFBundleVersion`, or `auto`, `git-count` or `timestamp`.
    pub build_number: Option<String>,
    pub deployment_target: Option<String>,
    pub devices: Vec<DeviceFamily>,
    pub orientations: BTreeMap<DeviceFamily, Vec<Orientation>>,
    pub icon: Option<String>,
    pub accent_color: Option<String>,
    pub launch_screen: Option<LaunchScreen>,
    pub info_plist: Option<String>,
    pub entitlements: Option<String>,
    pub privacy_manifest: Option<String>,
    pub resources: Vec<String>,
    pub extensions: Vec<AppExtension>,
}

impl Default for AppSection {
    fn default() -> Self {
        Self {
            bundle_id: None,
            display_name: None,
            version: None,
            build_number: None,
            deployment_target: None,
            devices: vec![DeviceFamily::Iphone],
            orientations: BTreeMap::new(),
            icon: None,
            accent_color: None,
            launch_screen: None,
            info_plist: None,
            entitlements: None,
            privacy_manifest: None,
            resources: Vec::new(),
            extensions: Vec::new(),
        }
    }
}

/// `[app] devices` entries and `orientations` keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceFamily {
    Iphone,
    Ipad,
}

/// `[app] orientations` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Orientation {
    Portrait,
    PortraitUpsideDown,
    LandscapeLeft,
    LandscapeRight,
}

impl Orientation {
    /// The `UIInterfaceOrientation…` value.
    pub fn plist_value(self) -> &'static str {
        match self {
            Orientation::Portrait => "UIInterfaceOrientationPortrait",
            Orientation::PortraitUpsideDown => "UIInterfaceOrientationPortraitUpsideDown",
            Orientation::LandscapeLeft => "UIInterfaceOrientationLandscapeLeft",
            Orientation::LandscapeRight => "UIInterfaceOrientationLandscapeRight",
        }
    }
}

/// `[app] launch-screen` (`UILaunchScreen`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct LaunchScreen {
    pub color: Option<String>,
    pub image: Option<String>,
    pub respects_safe_area: Option<bool>,
}

/// One `[[app.extensions]]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct AppExtension {
    /// The SwiftPM library product.
    pub product: String,
    /// For example `widgetkit`.
    pub kind: String,
    /// The extension's ID is `<app id>.<suffix>`; defaults to the product name, lowercased.
    pub bundle_id_suffix: Option<String>,
    pub info_plist: Option<String>,
    pub entitlements: Option<String>,
}

/// `[build]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct BuildSection {
    pub default_profile: String,
    pub sdk: String,
    pub backend: Backend,
    pub jobs: Jobs,
    pub metadata: MetadataName,
    pub metadata_consent: Option<MetadataConsent>,
    pub dsym: When,
    pub strip: When,
    pub lint: LintMode,
    pub launch_screen_fallback: LaunchScreenFallback,
    pub launch_storyboard: LaunchStoryboard,
    pub overrides: BTreeMap<String, String>,
    pub swiftpm: SwiftpmBuild,
    // W17 and W28 own these sections' semantics.
    pub flutter: Option<toml::Table>,
    pub xcode: Option<toml::Table>,
    pub profiles: BTreeMap<String, BuildProfile>,
}

impl Default for BuildSection {
    fn default() -> Self {
        Self {
            default_profile: "debug".into(),
            sdk: "auto".into(),
            backend: Backend::Swiftbuild,
            jobs: Jobs::Auto,
            metadata: MetadataName::Sdk,
            metadata_consent: None,
            dsym: When::Release,
            strip: When::Release,
            lint: LintMode::Auto,
            launch_screen_fallback: LaunchScreenFallback::Error,
            launch_storyboard: LaunchStoryboard::Auto,
            overrides: BTreeMap::new(),
            swiftpm: SwiftpmBuild::default(),
            flutter: None,
            xcode: None,
            profiles: BTreeMap::new(),
        }
    }
}

/// `[build] backend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    Swiftbuild,
    Native,
    Xtool,
    Pif,
}

impl Backend {
    /// The wire and file name.
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Swiftbuild => "swiftbuild",
            Backend::Native => "native",
            Backend::Xtool => "xtool",
            Backend::Pif => "pif",
        }
    }
}

/// `[build] jobs`: `"auto"` or a positive number.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Jobs {
    #[default]
    Auto,
    Count(u32),
}

impl<'de> Deserialize<'de> for Jobs {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Jobs;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("\"auto\" or a positive number")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Jobs, E> {
                if v == "auto" { Ok(Jobs::Auto) } else { Err(E::invalid_value(serde::de::Unexpected::Str(v), &self)) }
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Jobs, E> {
                match u32::try_from(v) {
                    Ok(n) if n > 0 => Ok(Jobs::Count(n)),
                    _ => Err(E::invalid_value(serde::de::Unexpected::Signed(v), &self)),
                }
            }
        }
        d.deserialize_any(V)
    }
}

/// `[build] metadata` (ARCHITECTURE §16.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetadataName {
    None,
    Minimal,
    Sdk,
    XcodeCompat,
}

/// `[build] metadata-consent`, required for `xcode-compat`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct MetadataConsent {
    pub by: String,
    pub at: String,
}

/// `never | release | always` (`[build] dsym`, `strip`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum When {
    Never,
    Release,
    Always,
}

/// `[build] lint`: `auto` warns in debug and errors in release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LintMode {
    Auto,
    Warn,
    Error,
    Off,
}

/// `[build] launch-screen-fallback`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchScreenFallback {
    Error,
    Empty,
}

/// `[build] launch-storyboard` [D 75].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchStoryboard {
    Auto,
    Compile,
    Convert,
}

/// `[build.swiftpm]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct SwiftpmBuild {
    /// The package directory, relative to the project root.
    pub package: String,
    /// The app's library product (needed when the package has several).
    pub product: Option<String>,
    pub swiftc_flags: Vec<String>,
    pub cc_flags: Vec<String>,
    pub ld_flags: Vec<String>,
}

impl Default for SwiftpmBuild {
    fn default() -> Self {
        Self {
            package: ".".into(),
            product: None,
            swiftc_flags: Vec::new(),
            cc_flags: Vec::new(),
            ld_flags: Vec::new(),
        }
    }
}

/// `[build.profiles.<name>]`: any `[build]` key overridden per profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct BuildProfile {
    pub sdk: Option<String>,
    pub backend: Option<Backend>,
    pub metadata: Option<MetadataName>,
    pub dsym: Option<When>,
    pub strip: Option<When>,
    pub lint: Option<LintMode>,
    pub package: Option<PackageKind>,
    pub overrides: BTreeMap<String, String>,
}

/// What to package after the build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKind {
    #[default]
    None,
    IpaDev,
    IpaStore,
}

/// The two `[signing]` keys the build plane reads; the rest of the section is the signing area's.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct SigningSection {
    /// `auto | exact | team-suffix | xtool-prefix | { custom = { … } }`.
    pub bundle_id_policy: Option<toml::Value>,
    pub team: Option<String>,
}

/// A project manifest and the files it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedManifest {
    pub manifest: ProjectManifest,
    /// The files that exist, lowest precedence first.
    pub files: Vec<Utf8PathBuf>,
    /// `[build] metadata-consent` as committed in `russet.toml` alone: `xcode-compat` needs a committed consent
    /// (ARCHITECTURE §16.4), so a consent only in the gitignored `russet.local.toml` does not count.
    pub committed_consent: Option<MetadataConsent>,
}

/// Loads `root/russet.toml` and `root/russet.local.toml` (either may be missing; both missing gives the defaults
/// and no files).
pub fn load(root: &Utf8Path) -> Result<LoadedManifest> {
    let mut merged = toml::Table::new();
    let mut files = Vec::new();
    let mut committed_consent = None;
    for name in [PROJECT_FILE, LOCAL_FILE] {
        let path = root.join(name);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(Error::new(codes::core::IO, format!("cannot read {path}: {e}"))
                    .with_hint("Check that the file is readable by your user.")
                    .caused_by(e));
            }
        };
        let checked: ProjectManifest = toml::from_str(&text).map_err(|e| config_error(&path, &text, &e))?;
        if name == PROJECT_FILE {
            committed_consent = checked.build.metadata_consent.clone();
        }
        if let Some(schema) = checked.schema
            && schema != 1
        {
            return Err(Error::new(codes::config::INVALID, format!("{path}: schema {schema} is not supported"))
                .with_hint("This Russet reads `schema = 1` project files; update Russet.")
                .with_details(serde_json::json!({"file": path})));
        }
        let table: toml::Table = toml::from_str(&text).map_err(|e| config_error(&path, &text, &e))?;
        merge(&mut merged, table);
        files.push(path);
    }
    let manifest = ProjectManifest::deserialize(merged).map_err(|e| {
        Error::new(codes::config::INVALID, format!("{}: {}", root.join(LOCAL_FILE), e.message()))
            .with_hint("russet.local.toml is layered over russet.toml; check the overridden keys.")
    })?;
    Ok(LoadedManifest { manifest, files, committed_consent })
}

/// Layers `over` onto `base`: tables merge key by key, anything else replaces.
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

/// `config.unknownKey` or `config.invalid` with `file:line:col`.
fn config_error(path: &Utf8Path, text: &str, e: &toml::de::Error) -> Error {
    let (line, column) = e.span().map_or((1, 1), |s| line_col(text, s.start));
    let message = e.message().trim();
    let code = if message.starts_with("unknown field") { codes::config::UNKNOWN_KEY } else { codes::config::INVALID };
    Error::new(code, format!("{path}:{line}:{column}: {message}"))
        .with_hint("Keys are kebab-case and follow ARCHITECTURE §14.2; fix the key or value at that position.")
        .with_details(serde_json::json!({"file": path, "line": line, "column": column}))
}

/// 1-based line and column of byte `offset` in `text`.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn dir(files: &[(&str, &str)]) -> (tempfile::TempDir, Utf8PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(d.path().to_path_buf()).unwrap();
        for (name, text) in files {
            std::fs::write(root.join(name), text).unwrap();
        }
        (d, root)
    }

    #[test]
    fn the_fixture_shape_parses_with_defaults() {
        let (_d, root) = dir(&[(
            PROJECT_FILE,
            "schema = 1\n[project]\nname = \"P\"\nstack = \"swiftpm\"\n[app]\nbundle-id = \"dev.russet.test.probe\"\n\
             deployment-target = \"17.0\"\ndevices = [\"iphone\", \"ipad\"]\n[build.swiftpm]\nproduct = \"P\"\n\
             [run]\nanything = 1\n[signing]\nmode = \"automatic\"\n",
        )]);
        let m = load(&root).unwrap();
        assert_eq!(m.files, vec![root.join(PROJECT_FILE)]);
        let m = m.manifest;
        assert_eq!(m.project.stack, Some(Stack::Swiftpm));
        assert_eq!(m.app.bundle_id.as_deref(), Some("dev.russet.test.probe"));
        assert_eq!(m.app.devices, vec![DeviceFamily::Iphone, DeviceFamily::Ipad]);
        assert_eq!(m.build.backend, Backend::Swiftbuild);
        assert_eq!(m.build.metadata, MetadataName::Sdk);
        assert_eq!(m.build.default_profile, "debug");
        assert_eq!(m.build.swiftpm.product.as_deref(), Some("P"));
        assert_eq!(m.signing.bundle_id_policy, None);
    }

    #[test]
    fn a_typo_reports_file_line_and_column() {
        let (_d, root) = dir(&[(PROJECT_FILE, "[app]\nbundle-id = \"a.b\"\nbundel-id = \"x\"\n")]);
        let e = load(&root).unwrap_err();
        assert_eq!(e.code(), codes::config::UNKNOWN_KEY);
        assert!(e.message().contains("russet.toml:3:1:"), "{}", e.message());
        let (_d, root) = dir(&[(PROJECT_FILE, "[build]\nbackend = \"gradle\"\n")]);
        let e = load(&root).unwrap_err();
        assert_eq!(e.code(), codes::config::INVALID);
        assert!(e.message().contains("russet.toml:2:"), "{}", e.message());
        let (_d, root) = dir(&[(PROJECT_FILE, "schema = 2\n")]);
        assert_eq!(load(&root).unwrap_err().code(), codes::config::INVALID);
        let (_d, root) = dir(&[(PROJECT_FILE, "[nonsense]\n")]);
        assert_eq!(load(&root).unwrap_err().code(), codes::config::UNKNOWN_KEY);
    }

    #[test]
    fn the_local_file_layers_over_the_project_file() {
        let (_d, root) = dir(&[
            (
                PROJECT_FILE,
                "[app]\nbundle-id = \"dev.russet.test.probe\"\nversion = \"1.0\"\n[build]\nmetadata = \"minimal\"\n",
            ),
            (LOCAL_FILE, "[app]\nbundle-id = \"com.example.mine\"\n[signing]\nbundle-id-policy = \"xtool-prefix\"\n"),
        ]);
        let m = load(&root).unwrap();
        assert_eq!(m.files.len(), 2);
        assert_eq!(m.manifest.app.bundle_id.as_deref(), Some("com.example.mine"));
        assert_eq!(m.manifest.app.version.as_deref(), Some("1.0"));
        assert_eq!(m.manifest.build.metadata, MetadataName::Minimal);
        assert_eq!(m.manifest.signing.bundle_id_policy, Some(toml::Value::String("xtool-prefix".into())));
        let (_d, root) = dir(&[(LOCAL_FILE, "[app]\nbogus = 1\n")]);
        let e = load(&root).unwrap_err();
        assert!(e.message().contains("russet.local.toml:2:1"), "{}", e.message());
    }

    #[test]
    fn only_the_committed_file_records_the_metadata_consent() {
        let consent = "[build]\nmetadata-consent = { by = \"owner\", at = \"2026-10-11\" }\n";
        let (_d, root) = dir(&[(PROJECT_FILE, "[build]\nmetadata = \"xcode-compat\"\n"), (LOCAL_FILE, consent)]);
        let m = load(&root).unwrap();
        assert!(m.manifest.build.metadata_consent.is_some(), "the layered view has it");
        assert_eq!(m.committed_consent, None, "but russet.local.toml's consent is not committed");
        let (_d, root) = dir(&[(PROJECT_FILE, consent)]);
        let m = load(&root).unwrap();
        assert_eq!(m.committed_consent.map(|c| c.by), Some("owner".to_owned()));
    }

    #[test]
    fn jobs_accept_auto_or_a_count() {
        let (_d, root) = dir(&[(PROJECT_FILE, "[build]\njobs = 4\n")]);
        assert_eq!(load(&root).unwrap().manifest.build.jobs, Jobs::Count(4));
        let (_d, root) = dir(&[(PROJECT_FILE, "[build]\njobs = 0\n")]);
        assert_eq!(load(&root).unwrap_err().code(), codes::config::INVALID);
    }

    #[test]
    fn no_files_give_the_defaults() {
        let (_d, root) = dir(&[]);
        let m = load(&root).unwrap();
        assert!(m.files.is_empty());
        assert_eq!(m.manifest, ProjectManifest::default());
    }
}
