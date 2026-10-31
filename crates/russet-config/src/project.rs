// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet.toml` and `russet.local.toml` (ARCHITECTURE §14.2) [D 15, D 16].
//!
//! Keys are kebab-case; unknown keys are errors with file:line:col (`config.unknownKey`). Every section has the
//! defaults of §14.2, so a missing section or key resolves to its default. `russet.local.toml` uses the same
//! schema and is layered over `russet.toml`, except `[agents]`, which project files can only narrow (§14.1).
//!
//! The area that consumes a section owns its semantics (build: `[project]`, `[app]`, `[build]`; signing:
//! `[signing]`, `[ship]`; run/debug: `[run]`, `[debug]`); this module owns the file format and its validation.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A project's configuration: `russet.toml`, with `russet.local.toml` layered over it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[schemars(title = "russet.toml", description = "A Russet project file (ARCHITECTURE §14.2).")]
pub struct ProjectConfig {
    /// The file-format version; only `1` exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    pub project: ProjectSection,
    pub app: AppSection,
    pub build: BuildSection,
    pub signing: SigningSection,
    pub run: RunSection,
    pub debug: DebugSection,
    pub ship: ShipSection,
    pub toolchain: ToolchainSection,
    /// May only narrow the user's `mcp-policy.toml` (ARCHITECTURE §7.5, §14.1).
    pub agents: AgentsSection,
}

/// `[project]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct ProjectSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Auto-detected when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<Stack>,
}

/// `[project] stack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Stack {
    Swiftpm,
    Flutter,
    Xcode,
    ReactNative,
    Maui,
}

/// `[app]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct AppSection {
    /// The canonical bundle ID; paid teams use it exactly (ARCHITECTURE §16.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// `CFBundleShortVersionString`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `CFBundleVersion`, or `auto`, `git-count` or `timestamp`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_number: Option<String>,
    /// Must be one of the SDK's `ValidDeploymentTargets`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment_target: Option<String>,
    pub devices: Vec<DeviceFamily>,
    pub orientations: BTreeMap<DeviceFamily, Vec<Orientation>>,
    /// An icon set in a main-bundle catalog, or a `.png` (development only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_screen: Option<LaunchScreen>,
    /// Extra keys merged at precedence level 3 (build §4.4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info_plist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entitlements: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceFamily {
    Iphone,
    Ipad,
}

/// `[app] orientations` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Orientation {
    Portrait,
    PortraitUpsideDown,
    LandscapeLeft,
    LandscapeRight,
}

/// `[app] launch-screen`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct LaunchScreen {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub respects_safe_area: Option<bool>,
}

/// One `[[app.extensions]]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct AppExtension {
    /// The SwiftPM library product.
    pub product: String,
    /// For example `widgetkit`.
    pub kind: String,
    /// The effective ID is `<app bundle id>.<suffix>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id_suffix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info_plist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entitlements: Option<String>,
}

/// `[build]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct BuildSection {
    pub default_profile: String,
    /// `auto`, an SDK canonical name (`iphoneos27.0`) or an Xcode build (`27A266a`).
    pub sdk: String,
    pub backend: BuildBackend,
    pub jobs: Jobs,
    pub metadata: MetadataPolicy,
    /// Required for `metadata = "xcode-compat"`; recorded by `russet ship stamp` or the IDE (ARCHITECTURE §16.4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_consent: Option<MetadataConsent>,
    pub dsym: When,
    pub strip: When,
    pub lint: LintMode,
    pub launch_screen_fallback: LaunchScreenFallback,
    pub launch_storyboard: LaunchStoryboard,
    /// Build-setting overrides, for example `RUSSET_LINK_SDK_VERSION = "deployment"` (diagnosis only).
    pub overrides: BTreeMap<String, String>,
    pub swiftpm: SwiftpmBuild,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flutter: Option<FlutterBuild>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xcode: Option<XcodeBuild>,
    pub profiles: BTreeMap<String, BuildProfile>,
}

impl Default for BuildSection {
    fn default() -> Self {
        Self {
            default_profile: "debug".into(),
            sdk: "auto".into(),
            backend: BuildBackend::Swiftbuild,
            jobs: Jobs::Auto,
            metadata: MetadataPolicy::Sdk,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BuildBackend {
    Swiftbuild,
    Native,
    Xtool,
    Pif,
}

/// `[build] jobs`: `"auto"` or a positive number.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Jobs {
    #[default]
    Auto,
    Count(u32),
}

impl Serialize for Jobs {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => s.serialize_str("auto"),
            Self::Count(n) => s.serialize_u32(*n),
        }
    }
}

impl<'de> Deserialize<'de> for Jobs {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Jobs;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("\"auto\" or a positive number")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Jobs, E> {
                if v == "auto" { Ok(Jobs::Auto) } else { Err(E::invalid_value(serde::de::Unexpected::Str(v), &self)) }
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Jobs, E> {
                match u32::try_from(v) {
                    Ok(n) if n > 0 => Ok(Jobs::Count(n)),
                    _ => Err(E::invalid_value(serde::de::Unexpected::Signed(v), &self)),
                }
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Jobs, E> {
                match u32::try_from(v) {
                    Ok(n) if n > 0 => Ok(Jobs::Count(n)),
                    _ => Err(E::invalid_value(serde::de::Unexpected::Unsigned(v), &self)),
                }
            }
        }
        d.deserialize_any(V)
    }
}

impl JsonSchema for Jobs {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Jobs".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "\"auto\" or a positive number of parallel jobs.",
            "oneOf": [{"type": "string", "const": "auto"}, {"type": "integer", "minimum": 1}]
        })
    }
}

/// `[build] metadata` (ARCHITECTURE §16.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MetadataPolicy {
    None,
    Minimal,
    Sdk,
    XcodeCompat,
}

/// `[build] metadata-consent`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct MetadataConsent {
    pub by: String,
    pub at: String,
}

/// `never | release | always` (`[build] dsym`, `strip`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum When {
    Never,
    Release,
    Always,
}

/// `[build] lint`: `auto` warns in debug and errors in release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LintMode {
    Auto,
    Warn,
    Error,
    Off,
}

/// `[build] launch-screen-fallback`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchScreenFallback {
    Error,
    Empty,
}

/// `[build] launch-storyboard` [D 75].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchStoryboard {
    Auto,
    Compile,
    Convert,
}

/// `[build.swiftpm]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct SwiftpmBuild {
    pub package: String,
    #[serde(skip_serializing_if = "Option::is_none")]
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

/// `[build.flutter]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct FlutterBuild {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub target: String,
    pub flavor: String,
    pub dart_defines: Vec<String>,
    pub plugins: FlutterPlugins,
    pub gen_snapshot: String,
}

impl Default for FlutterBuild {
    fn default() -> Self {
        Self {
            version: None,
            target: "lib/main.dart".into(),
            flavor: String::new(),
            dart_defines: Vec::new(),
            plugins: FlutterPlugins::Swiftpm,
            gen_snapshot: "auto".into(),
        }
    }
}

/// `[build.flutter] plugins`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum FlutterPlugins {
    #[serde(rename = "swiftpm")]
    Swiftpm,
    #[serde(rename = "swiftpm+convert")]
    SwiftpmConvert,
    #[serde(rename = "swiftpm+cocoapods")]
    SwiftpmCocoapods,
}

/// `[build.xcode]` (Wave 6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct XcodeBuild {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// Russet profile → Xcode configuration, for example `{ debug = "Debug", release = "Release" }`.
    pub configuration: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cocoapods: Option<String>,
}

/// `[build.profiles.<name>]`: overrides of `[build]` for one profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct BuildProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<BuildBackend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsym: Option<When>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strip: Option<When>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lint: Option<LintMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageKind>,
    pub overrides: BTreeMap<String, String>,
}

/// `[build.profiles.<name>] package`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKind {
    None,
    IpaDev,
    IpaStore,
}

/// `[signing]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct SigningSection {
    /// An account id from `accounts.toml`; normally set in `russet.local.toml`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    pub mode: SigningMode,
    pub bundle_id_policy: BundleIdPolicy,
    pub merge: EntitlementMerge,
    /// Per-bundle settings, keyed by the bundle's path inside the app (for example `PlugIns/HelloWidget.appex`).
    pub targets: BTreeMap<String, SigningTarget>,
}

impl Default for SigningSection {
    fn default() -> Self {
        Self {
            account: None,
            team: None,
            mode: SigningMode::Automatic,
            bundle_id_policy: BundleIdPolicy::Named(BundleIdPolicyName::Auto),
            merge: EntitlementMerge::Least,
            targets: BTreeMap::new(),
        }
    }
}

/// `[signing] mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SigningMode {
    Automatic,
    Manual,
}

/// `[signing] bundle-id-policy` (ARCHITECTURE §16.5): a name, or `{ custom = { "<id>" = "<effective id>" } }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum BundleIdPolicy {
    Named(BundleIdPolicyName),
    Custom(CustomBundleIds),
}

/// The named bundle-ID policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BundleIdPolicyName {
    /// Exact for paid teams, team-suffix for free teams.
    Auto,
    Exact,
    TeamSuffix,
    XtoolPrefix,
}

/// `{ custom = { … } }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CustomBundleIds {
    pub custom: BTreeMap<String, String>,
}

/// `[signing] merge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EntitlementMerge {
    Least,
    ProfileAll,
}

/// `[signing.targets."<path>"]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct SigningTarget {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entitlements: Option<String>,
    /// Manual mode only: the path of a `.mobileprovision`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// `[run]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct RunSection {
    /// `auto`, a UDID or an alias.
    pub device: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub console: bool,
    pub install: InstallMode,
    pub launch: LaunchMode,
}

impl Default for RunSection {
    fn default() -> Self {
        Self {
            device: "auto".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            console: true,
            install: InstallMode::IfChanged,
            launch: LaunchMode::Auto,
        }
    }
}

/// `[run] install`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum InstallMode {
    IfChanged,
    Always,
    Never,
}

/// `[run] launch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchMode {
    Auto,
    Coredevice,
    Dvt,
}

/// `[debug]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct DebugSection {
    /// `auto`, `swift`, `llvm` or a path (RD §8.1).
    pub lldb: String,
    pub init_commands: Vec<String>,
    pub symbols: SymbolsMode,
    pub jit: JitMode,
    pub swift_expressions: AutoOnOff,
    pub attach_mode: AttachMode,
}

impl Default for DebugSection {
    fn default() -> Self {
        Self {
            lldb: "auto".into(),
            init_commands: Vec::new(),
            symbols: SymbolsMode::Auto,
            jit: JitMode::Auto,
            swift_expressions: AutoOnOff::Auto,
            attach_mode: AttachMode::Broker,
        }
    }
}

/// `[debug] symbols`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolsMode {
    Auto,
    None,
    Core,
    Full,
}

/// `[debug] jit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum JitMode {
    Auto,
    Native,
    LldbPython,
    Off,
}

/// `auto | on | off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AutoOnOff {
    Auto,
    On,
    Off,
}

/// `[debug] attach-mode` [D 38].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AttachMode {
    Broker,
    LegacyAsync,
}

/// `[ship]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct ShipSection {
    pub mode: ShipMode,
    /// The numeric App Store Connect app id; filled on the first upload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    pub build_number: String,
    pub testflight_groups: Vec<String>,
    /// Written as `ITSAppUsesNonExemptEncryption`.
    pub uses_non_exempt_encryption: bool,
    pub ci: ShipCi,
}

impl Default for ShipSection {
    fn default() -> Self {
        Self {
            mode: ShipMode::LinuxNative,
            app_id: None,
            build_number: "auto".into(),
            testflight_groups: vec!["Internal".into()],
            uses_non_exempt_encryption: false,
            ci: ShipCi::default(),
        }
    }
}

/// `[ship] mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ShipMode {
    LinuxNative,
    AppleHardware,
}

/// `[ship.ci]` [D 76].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct ShipCi {
    pub provider: CiProvider,
    pub signing: CiSigning,
}

impl Default for ShipCi {
    fn default() -> Self {
        Self { provider: CiProvider::Github, signing: CiSigning::Russet }
    }
}

/// `[ship.ci] provider`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CiProvider {
    Github,
    XcodeCloud,
}

/// `[ship.ci] signing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CiSigning {
    Russet,
    XcodeManual,
    XcodeCloud,
}

/// `[toolchain]`: overrides of the pin manifest (ARCHITECTURE §16.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct ToolchainSection {
    /// For example `6.4.0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swift: Option<String>,
    /// Needs an imported SDK of this Xcode version, for example `27.0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xcode_sdk: Option<String>,
    /// `auto`, `darwin-tools@1.1.0`, `llvm@>=23.1.3` or a path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linker: Option<String>,
}

/// `[agents]`: may only narrow `mcp-policy.toml` (ARCHITECTURE §7.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct AgentsSection {
    /// Lowers every client's `tier-max`; it never raises one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 3))]
    pub tier_max: Option<u8>,
    /// MCP tools agents may not call in this project.
    pub deny_tools: Vec<String>,
}
