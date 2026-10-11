// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The build orchestrator (ARCHITECTURE §5.3 `russet-build`; build §3.2, §9.1): project detection, stack adapters
//! (SwiftPM now; Flutter, Xcode, React Native, MAUI later), Info.plist synthesis, bundle assembly, rpath hygiene,
//! the metadata policy, IPA packaging and `build-report.json`. Events are `JobEvent` kinds through
//! `russet_core::progress::EventSink`, mirrored to `<project>/.russet/logs/<jobId>.jsonl`.
//!
//! ```text
//! BuildRequest → resolve (project, russet.toml, SDK facts, Swift ↔ SDK lock, bundle IDs)
//!   → prepare (swift package resolve/show-dependencies/describe, the builder package)
//!   → compile (swift build --build-system swiftbuild with the SDK's toolset and platform view)
//!   → resources → assemble (.app, Info.plist, DT keys, rpath hygiene) → lint → package (IPA)
//!   → BuildOutput + build-report.json + SigningPlan (handed to the signing area)
//! ```
//!
//! W5 owns the crate in Wave 1, W16 in Wave 2; the stack, resource and platform modules belong to the cards
//! listed in PLAN §3.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use plist::{Dictionary, Value};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::ids::JobId;
use russet_core::proc::{CommandRunner, CommandSpec};
use russet_core::progress::EventSink;
use russet_plist::macros::SettingsStack;
use russet_sdk::{SdkFacts, SdkProvider, SdkSelector, Version};
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub mod bundle;
pub mod bundle_id;
pub mod detect;
pub mod events;
pub mod infoplist;
pub mod inspect;
pub mod macho;
pub mod manifest;
pub mod metadata;
pub mod platform;
pub mod report;
pub mod resources;
pub mod stack;

pub use camino::{Utf8Path, Utf8PathBuf};
pub use events::{BuildEvent, NullSink, Stage};
pub use macho::{MachOSummary, MachOTool, RussetMachO};
pub use manifest::{Backend, PackageKind, Stack};

use crate::bundle::{AssemblyInput, ProductBundle};
use crate::events::{ArtifactKind, Emitter, StageState, TaskState, warn};
use crate::manifest::{LoadedManifest, MetadataName, ProjectManifest, When};
use crate::stack::swiftpm::{self as spm, PackageGraph};

/// What a [`Builder`] needs from its environment (build §9.1).
#[derive(Clone)]
pub struct BuildEnv {
    pub sdk: Arc<dyn SdkProvider>,
    /// `russet-tool` multi-call links (`xcrun`, `actool`, …).
    pub tools_dir: Utf8PathBuf,
    pub data_dir: Utf8PathBuf,
    pub cache_dir: Utf8PathBuf,
    /// Runs `swift` and the other tools: W2's `ProcessRunner` in the daemon.
    pub runner: Arc<dyn CommandRunner>,
    /// Mach-O reads and in-place edits: `russet-macho` (W6) unless a test substitutes one.
    pub macho: Arc<dyn MachOTool>,
    /// The fixed modification time of bundle files and IPA entries (`SOURCE_DATE_EPOCH`, else 2001-01-01).
    pub epoch: SystemTime,
}

impl BuildEnv {
    /// An environment with the default Mach-O tool, epoch and tools directory (`data/tools/<version>/bin`).
    pub fn new(
        sdk: Arc<dyn SdkProvider>,
        data_dir: impl Into<Utf8PathBuf>,
        cache_dir: impl Into<Utf8PathBuf>,
        runner: Arc<dyn CommandRunner>,
    ) -> Self {
        let data_dir = data_dir.into();
        Self {
            sdk,
            tools_dir: data_dir.join("tools").join(env!("CARGO_PKG_VERSION")).join("bin"),
            data_dir,
            cache_dir: cache_dir.into(),
            runner,
            macho: Arc::new(RussetMachO),
            epoch: russet_ipa::default_epoch(),
        }
    }

    /// Replaces the Mach-O tool.
    pub fn with_macho(mut self, macho: Arc<dyn MachOTool>) -> Self {
        self.macho = macho;
        self
    }

    /// Replaces the fixed timestamp.
    pub fn with_epoch(mut self, epoch: SystemTime) -> Self {
        self.epoch = epoch;
        self
    }
}

/// What a built bundle is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BundleKind {
    App,
    AppExtension,
}

/// One target of a project (`project.info`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTarget {
    pub name: String,
    /// The canonical bundle ID from `russet.toml` (`None` until `[app] bundle-id` is set).
    pub bundle_id: Option<String>,
    pub kind: BundleKind,
}

/// A detected project (`project.detect`, `project.info`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub root: Utf8PathBuf,
    pub stack: Stack,
    pub name: String,
    /// `russet.toml`, `russet.local.toml` and the stack's manifest files.
    pub manifests: Vec<Utf8PathBuf>,
    /// Products the project declares (for SwiftPM, its library products).
    pub products: Vec<String>,
    pub targets: Vec<ProjectTarget>,
    pub configurations: Vec<String>,
    pub default_target: Option<String>,
}

/// A build request (`build.start` params).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    /// The project root or any directory inside the project.
    pub project: Utf8PathBuf,
    /// The app product; `[build.swiftpm] product` when omitted.
    pub target: Option<String>,
    /// `debug`, `release`, `profile` or a `[build.profiles.<name>]`; `[build] default-profile` when omitted.
    pub configuration: Option<String>,
    /// The signing area's policy; `[signing] bundle-id-policy` when omitted.
    pub bundle_id_policy: Option<BundleIdPolicy>,
    /// Build-setting overrides, on top of `[build] overrides`.
    pub overrides: BTreeMap<String, String>,
    pub clean: bool,
    /// What to package; the profile's `package` when omitted.
    pub package: Option<PackageKind>,
    pub backend: Option<Backend>,
    /// The daemon's job id; a fresh one when omitted.
    pub job_id: Option<JobId>,
}

impl BuildRequest {
    /// A request for the project at `project` with every option at its default.
    pub fn new(project: impl Into<Utf8PathBuf>) -> Self {
        Self {
            project: project.into(),
            target: None,
            configuration: None,
            bundle_id_policy: None,
            overrides: BTreeMap::new(),
            clean: false,
            package: None,
            backend: None,
            job_id: None,
        }
    }
}

/// A dry-run plan (`build.plan`): what a build would do, without running anything.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildPlan {
    pub root: Utf8PathBuf,
    pub stack: Stack,
    /// The app product (`None` until the package is described, when neither the request nor `russet.toml` names it).
    pub product: Option<String>,
    pub extensions: Vec<String>,
    pub configuration: String,
    pub release: bool,
    pub backend: Backend,
    pub package: PackageKind,
    pub metadata: MetadataPolicy,
    pub sdk: SdkFacts,
    /// `[app] deployment-target`, else the `.iOS` platform a scan of `Package.swift` finds (`None` when neither
    /// says; a build reads the platform from `swift package describe`).
    pub deployment_target: Option<Version>,
    /// Original → effective bundle IDs.
    pub bundle_ids: BTreeMap<String, String>,
    /// Where the app will be.
    pub app_path: Option<Utf8PathBuf>,
    pub report_path: Utf8PathBuf,
    pub stages: Vec<Stage>,
    pub warnings: Vec<String>,
}

/// What `build.clean` removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanScope {
    /// `.russet/work` and `.russet/build`.
    Build,
    /// All of `.russet`.
    All,
}

/// What `build.clean` freed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanReport {
    pub freed_bytes: u64,
}

/// How effective bundle IDs are derived (ARCHITECTURE §16.5) [D 18].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleIdPolicy {
    Exact,
    TeamSuffix { team_id: String },
    XtoolPrefix { prefix: String },
    Custom(BTreeMap<String, String>),
}

/// A recorded consent for `xcode-compat` metadata (`[build] metadata-consent = {by, at}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consent {
    pub by: String,
    pub at: String,
}

/// `Info.plist` DT* key policy (ARCHITECTURE §16.4) [D 17].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataPolicy {
    None,
    Minimal,
    Sdk,
    XcodeCompat { consent: Consent },
}

impl MetadataPolicy {
    /// The `[build] metadata` name.
    pub fn as_str(&self) -> &'static str {
        match self {
            MetadataPolicy::None => "none",
            MetadataPolicy::Minimal => "minimal",
            MetadataPolicy::Sdk => "sdk",
            MetadataPolicy::XcodeCompat { .. } => "xcode-compat",
        }
    }
}

/// One built bundle: path, executable, effective and original bundle IDs, versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleInfo {
    pub path: Utf8PathBuf,
    /// The effective `CFBundleIdentifier`.
    pub bundle_id: String,
    /// The canonical ID before the policy.
    pub original_bundle_id: String,
    pub executable: Utf8PathBuf,
    pub kind: BundleKind,
    pub product: String,
    /// `CFBundleShortVersionString`.
    pub version: String,
    /// `CFBundleVersion`.
    pub build_number: String,
}

/// What a nested item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignKind {
    Bundle,
    MachO,
}

/// One item to sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignItem {
    pub path: Utf8PathBuf,
    pub kind: SignKind,
    pub entitlements: Option<Utf8PathBuf>,
    pub get_task_allow: bool,
}

/// Items to sign, inside-out: deepest first, app last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SigningPlan {
    pub items: Vec<SignItem>,
}

/// Lint severities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LintSeverity {
    Info,
    Warning,
    Error,
}

/// A build lint finding (build §4.7; the full catalog is W16's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LintFinding {
    /// `L001` … `L012`.
    #[serde(rename = "id")]
    pub code: String,
    pub severity: LintSeverity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Utf8PathBuf>,
    pub message: String,
    /// Whether the build fixed it (rpath hygiene fixes L006).
    pub fixed: bool,
}

/// The result of `build.start` (ARCHITECTURE §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutput {
    pub build_id: JobId,
    pub app: BundleInfo,
    pub extensions: Vec<BundleInfo>,
    /// Original → effective bundle IDs.
    pub bundle_ids: BTreeMap<String, String>,
    pub signing_plan: SigningPlan,
    pub dsyms: Vec<Utf8PathBuf>,
    pub ipa: Option<Utf8PathBuf>,
    pub report_path: Utf8PathBuf,
    pub lints: Vec<LintFinding>,
    pub duration: Duration,
    /// What russet-debug needs (UUIDs, SDK, Swift module directories).
    pub built: BuiltApp,
}

/// What russet-debug consumes from a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltApp {
    pub app: Utf8PathBuf,
    pub executable: Utf8PathBuf,
    pub uuids: Vec<Uuid>,
    pub dsyms: Vec<Utf8PathBuf>,
    pub sdk_path: Utf8PathBuf,
    pub swift_module_dirs: Vec<Utf8PathBuf>,
    pub flutter_framework: Option<Utf8PathBuf>,
}

/// A fresh job id (`job_` plus 26 Crockford base32 characters: 48 bits of milliseconds, 80 random bits).
///
/// Until W2's `JobId::generate` lands (freeze F1) the id is built here and parsed through serde.
pub fn new_job_id() -> JobId {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64) & ((1 << 48) - 1);
    let mut random = 0u128;
    for round in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(round);
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u32(std::process::id());
        h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        random = (random << 64) | u128::from(h.finish());
    }
    let value: u128 = (u128::from(ms) << 80) | (random & ((1u128 << 80) - 1));
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut text = String::with_capacity(30);
    text.push_str("job_");
    for i in (0..26).rev() {
        text.push(char::from(ALPHABET[((value >> (i * 5)) & 31) as usize]));
    }
    serde_json::from_value(serde_json::Value::String(text)).unwrap_or_else(|_| unreachable_job_id())
}

#[cold]
fn unreachable_job_id() -> JobId {
    // A newtype over String always deserializes from a string.
    serde_json::from_str("\"job_00000000000000000000000000\"")
        .unwrap_or_else(|_| unreachable!("JobId is a string newtype"))
}

/// The configuration of a build: its profile name, whether it is a release build, and the merged `[build]` keys.
#[derive(Debug, Clone)]
struct Profile {
    name: String,
    release: bool,
    sdk: String,
    backend: Backend,
    metadata: MetadataName,
    dsym: When,
    strip: When,
    lint: manifest::LintMode,
    package: PackageKind,
    overrides: BTreeMap<String, String>,
}

fn profile(m: &ProjectManifest, requested: Option<&str>) -> Result<Profile> {
    let name = requested.map(str::to_owned).unwrap_or_else(|| m.build.default_profile.clone());
    let known = matches!(name.as_str(), "debug" | "release" | "profile");
    let section = m.build.profiles.get(&name);
    if !known && section.is_none() {
        return Err(Error::new(codes::config::INVALID, format!("no build profile `{name}`"))
            .with_hint("Use debug, release or profile, or define `[build.profiles.<name>]` in russet.toml."));
    }
    let b = &m.build;
    let p = section.cloned().unwrap_or_default();
    let mut overrides = b.overrides.clone();
    overrides.extend(p.overrides.clone());
    Ok(Profile {
        release: name != "debug",
        sdk: p.sdk.unwrap_or_else(|| b.sdk.clone()),
        backend: p.backend.unwrap_or(b.backend),
        metadata: p.metadata.unwrap_or(b.metadata),
        dsym: p.dsym.unwrap_or(b.dsym),
        strip: p.strip.unwrap_or(b.strip),
        lint: p.lint.unwrap_or(b.lint),
        package: p.package.unwrap_or_default(),
        overrides,
        name,
    })
}

/// The metadata policy of a profile. `xcode-compat` needs `[build] metadata-consent = {by, at}` committed in
/// `russet.toml` (ARCHITECTURE §16.4); a consent that only `russet.local.toml` gives does not count.
fn metadata_policy(loaded: &LoadedManifest, name: MetadataName) -> Result<MetadataPolicy> {
    Ok(match name {
        MetadataName::None => MetadataPolicy::None,
        MetadataName::Minimal => MetadataPolicy::Minimal,
        MetadataName::Sdk => MetadataPolicy::Sdk,
        MetadataName::XcodeCompat => {
            let committed =
                loaded.committed_consent.clone().filter(|c| !c.by.trim().is_empty() && !c.at.trim().is_empty());
            let Some(c) = committed else {
                let message = if loaded.manifest.build.metadata_consent.is_some() {
                    "metadata = \"xcode-compat\" needs its consent committed in russet.toml; russet.local.toml's does not count"
                } else {
                    "metadata = \"xcode-compat\" needs a recorded consent"
                };
                return Err(Error::new(codes::config::INVALID, message).with_hint(
                    "Commit `[build] metadata-consent = { by = \"<name>\", at = \"<date>\" }` in russet.toml (ARCHITECTURE §16.4).",
                ));
            };
            MetadataPolicy::XcodeCompat { consent: Consent { by: c.by, at: c.at } }
        }
    })
}

/// The app's `CFBundleShortVersionString` and `CFBundleVersion` as synthesized (`russet.toml`, or the project's own
/// Info.plist when it sets them), falling back to the given values.
fn effective_versions(info: &Dictionary, version: String, build: String) -> (String, String) {
    let get = |key: &str| info.get(key).and_then(Value::as_string).map(str::to_owned);
    (get("CFBundleShortVersionString").unwrap_or(version), get("CFBundleVersion").unwrap_or(build))
}

fn when(w: When, release: bool) -> bool {
    match w {
        When::Never => false,
        When::Release => release,
        When::Always => true,
    }
}

/// `(product, suffix)` of every `[[app.extensions]]`.
fn extension_ids(m: &ProjectManifest) -> Vec<(String, String)> {
    m.app
        .extensions
        .iter()
        .map(|e| (e.product.clone(), e.bundle_id_suffix.clone().unwrap_or_else(|| e.product.to_lowercase())))
        .collect()
}

fn require_bundle_id(m: &ProjectManifest, root: &Utf8Path) -> Result<String> {
    m.app.bundle_id.clone().filter(|b| !b.is_empty()).ok_or_else(|| {
        Error::new(codes::config::INVALID, format!("{} has no `[app] bundle-id`", root.join(manifest::PROJECT_FILE)))
            .with_hint("Add `[app] bundle-id = \"com.example.app\"` to russet.toml (ARCHITECTURE §14.2).")
    })
}

fn require_swiftpm(stack: Stack) -> Result<()> {
    let wave = match stack {
        Stack::Swiftpm => return Ok(()),
        Stack::Flutter => "Flutter builds land in Wave 3 (W17)",
        Stack::Xcode => "Xcode-project builds land in Wave 6 (W28)",
        Stack::ReactNative => "React Native builds land in Wave 6 (W29)",
        Stack::Maui => ".NET MAUI is a study for now (W29)",
    };
    Err(Error::new(codes::core::NOT_IMPLEMENTED, format!("this is a {} project; {wave}", stack.as_str()))
        .with_hint("Russet builds SwiftPM apps today (PLAN §1)."))
}

/// The Swift ↔ SDK lock (ARCHITECTURE §16.1): the toolchain's minor version must be the SDK's.
fn check_swift(facts: &SdkFacts) -> Result<()> {
    match facts.tools.swift_version {
        Some(v) if v.same_minor(facts.swift_required) => Ok(()),
        Some(v) => Err(Error::new(
            codes::sdk::INCOMPATIBLE,
            format!(
                "the Xcode {} SDK needs Swift {}, but the toolchain is Swift {v}",
                facts.xcode_version, facts.swift_required
            ),
        )
        .with_hint(format!(
            "Install Swift {} (`russet toolchain list` shows what Russet found).",
            facts.swift_required
        ))),
        None => Err(Error::new(codes::toolchain::BROKEN, format!("cannot tell the version of {}", facts.tools.swift))
            .with_hint("Check that `swift --version` runs, then `russet doctor`.")),
    }
}

/// The deployment target: `[app] deployment-target`, else the package's `.iOS` platform, else 15.0, checked
/// against the SDK's `ValidDeploymentTargets`.
fn deployment_target(m: &ProjectManifest, package: Option<Version>, facts: &SdkFacts) -> Result<Version> {
    let dt = match &m.app.deployment_target {
        Some(s) => Version::parse(s).ok_or_else(|| {
            Error::new(codes::config::INVALID, format!("`[app] deployment-target = \"{s}\"` is not a version"))
                .with_hint("Use a version such as \"17.0\".")
        })?,
        None => package.unwrap_or(spm::DEFAULT_DEPLOYMENT_TARGET),
    };
    if !facts.valid_deployment_targets.is_empty() && !facts.accepts_deployment_target(dt) {
        let valid: Vec<String> = facts.valid_deployment_targets.iter().map(Version::to_string).collect();
        return Err(Error::new(
            codes::config::INVALID,
            format!(
                "deployment target {dt} is not one of the {} SDK's ValidDeploymentTargets",
                facts.sdk_canonical_name
            ),
        )
        .with_hint(format!("Use one of: {}.", valid.join(", "))));
    }
    Ok(dt)
}

/// A lock on `<project>/.russet/work/build.lock` for the whole build (`build.inProgress` when held).
fn lock_project(work: &Utf8Path) -> Result<File> {
    std::fs::create_dir_all(work).map_err(|e| spm::io(work, e))?;
    let path = work.join("build.lock");
    let file = File::options().create(true).truncate(false).write(true).open(&path).map_err(|e| spm::io(&path, e))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(Error::new(codes::build::IN_PROGRESS, "another build of this project is running")
                .with_hint("Wait for it to finish, or cancel it with `russet job cancel` / Ctrl-C.")
                .retryable(true))
        }
        Err(std::fs::TryLockError::Error(e)) => Err(spm::io(&path, e)),
    }
}

const DEPS_HINT: &str = "Check that `swift package show-dependencies --format json` works in the project, then \
                         rebuild with `russet build --clean`.";

/// The build orchestrator.
pub struct Builder {
    env: BuildEnv,
}

/// Everything resolved before planning the package.
struct Resolved {
    root: Utf8PathBuf,
    loaded: LoadedManifest,
    stack: Stack,
    profile: Profile,
    backend: Backend,
    package: PackageKind,
    metadata: MetadataPolicy,
    overrides: BTreeMap<String, String>,
    facts: SdkFacts,
    ids: bundle_id::BundleIds,
    app_bundle_id: String,
}

impl Resolved {
    fn m(&self) -> &ProjectManifest {
        &self.loaded.manifest
    }

    fn russet_dir(&self) -> Utf8PathBuf {
        self.root.join(".russet")
    }

    fn work_dir(&self) -> Utf8PathBuf {
        self.russet_dir().join("work")
    }

    fn out_dir(&self) -> Utf8PathBuf {
        self.russet_dir().join("build").join(format!("{}-iphoneos", self.profile.name))
    }

    fn package_dir(&self) -> Utf8PathBuf {
        let p = &self.m().build.swiftpm.package;
        if p == "." || p.is_empty() { self.root.clone() } else { self.root.join(p) }
    }
}

impl Builder {
    pub fn new(env: BuildEnv) -> Result<Self> {
        Ok(Self { env })
    }

    /// The environment.
    pub fn env(&self) -> &BuildEnv {
        &self.env
    }

    /// Detects the project at or above `dir` (`project.detect`, `project.info`).
    pub async fn detect(&self, dir: &Utf8Path) -> Result<ProjectInfo> {
        let root = detect::find_root(dir)?;
        let loaded = manifest::load(&root)?;
        let m = &loaded.manifest;
        let (stack, markers) = detect::stack(&root, m.project.stack)?;
        let mut manifests = loaded.files.clone();
        manifests.extend(markers);
        let mut products = Vec::new();
        if stack == Stack::Swiftpm {
            let pkg = if m.build.swiftpm.package == "." { root.clone() } else { root.join(&m.build.swiftpm.package) };
            products = detect::swiftpm_products(&pkg.join("Package.swift"));
        }
        let app_product = m.build.swiftpm.product.clone().or_else(|| match products.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        });
        let mut targets = Vec::new();
        if let Some(app) = &app_product {
            targets.push(ProjectTarget {
                name: app.clone(),
                bundle_id: m.app.bundle_id.clone(),
                kind: BundleKind::App,
            });
        }
        for (product, suffix) in extension_ids(m) {
            let bundle_id = m.app.bundle_id.as_ref().map(|id| format!("{id}.{suffix}"));
            targets.push(ProjectTarget { name: product, bundle_id, kind: BundleKind::AppExtension });
        }
        let mut configurations = vec!["debug".to_owned(), "release".to_owned(), "profile".to_owned()];
        configurations
            .extend(m.build.profiles.keys().filter(|k| !configurations.contains(k)).cloned().collect::<Vec<_>>());
        let name = m
            .project
            .name
            .clone()
            .or_else(|| app_product.clone())
            .unwrap_or_else(|| root.file_name().unwrap_or("project").to_owned());
        Ok(ProjectInfo { root, stack, name, manifests, products, targets, configurations, default_target: app_product })
    }

    async fn resolve(&self, r: &BuildRequest) -> Result<Resolved> {
        let root = detect::find_root(&r.project)?;
        let loaded = manifest::load(&root)?;
        let (stack, _) = detect::stack(&root, loaded.manifest.project.stack)?;
        require_swiftpm(stack)?;
        let m = &loaded.manifest;
        let profile = profile(m, r.configuration.as_deref())?;
        let backend = r.backend.unwrap_or(profile.backend);
        let package = r.package.unwrap_or(profile.package);
        let metadata = metadata_policy(&loaded, profile.metadata)?;
        let mut overrides = profile.overrides.clone();
        overrides.extend(r.overrides.clone());
        let app_bundle_id = require_bundle_id(m, &root)?;
        let policy = bundle_id::policy_for(r.bundle_id_policy.as_ref(), &m.signing)?;
        let ids = bundle_id::apply(&policy, &app_bundle_id, &extension_ids(m))?;
        let facts = self.env.sdk.resolve(&SdkSelector::parse(&profile.sdk)).await?;
        Ok(Resolved { root, loaded, stack, profile, backend, package, metadata, overrides, facts, ids, app_bundle_id })
    }

    /// Whether a build of `r` needs the signing account's xtool identity prefix (ARCHITECTURE §16.5): the request
    /// names no policy and the project's `[signing] bundle-id-policy` is `xtool-prefix` (as in PLAN §4.10's
    /// `russet.local.toml`). The daemon then reads the prefix from its account service and passes
    /// [`BundleIdPolicy::XtoolPrefix`]. `false` when the project cannot be read: the build itself reports why.
    pub fn needs_xtool_prefix(&self, r: &BuildRequest) -> bool {
        r.bundle_id_policy.is_none()
            && detect::find_root(&r.project)
                .and_then(|root| manifest::load(&root))
                .is_ok_and(|l| bundle_id::is_xtool_prefix(&l.manifest.signing))
    }

    /// A dry run: resolves the project, profile, SDK and bundle IDs without building.
    pub async fn plan(&self, r: &BuildRequest) -> Result<BuildPlan> {
        let res = self.resolve(r).await?;
        let m = res.m();
        let mut warnings = Vec::new();
        if let Err(e) = check_swift(&res.facts) {
            warnings.push(e.message().to_owned());
        }
        let deployment_target = m
            .app
            .deployment_target
            .as_deref()
            .and_then(Version::parse)
            .or_else(|| detect::swiftpm_ios_platform(&res.package_dir().join("Package.swift")));
        let product = r.target.clone().or_else(|| m.build.swiftpm.product.clone());
        let mut stages = Stage::ALL.to_vec();
        if res.package == PackageKind::None {
            stages.retain(|s| *s != Stage::Package);
        }
        let out = res.out_dir();
        Ok(BuildPlan {
            app_path: product.as_ref().map(|p| out.join(format!("{p}.app"))),
            report_path: out.join("build-report.json"),
            root: res.root.clone(),
            stack: res.stack,
            extensions: m.app.extensions.iter().map(|e| e.product.clone()).collect(),
            configuration: res.profile.name.clone(),
            release: res.profile.release,
            backend: res.backend,
            package: res.package,
            metadata: res.metadata.clone(),
            bundle_ids: res.ids.map(),
            sdk: res.facts.clone(),
            product,
            deployment_target,
            stages,
            warnings,
        })
    }

    /// Builds the app.
    ///
    /// Events go to `sink` (and to `<project>/.russet/logs/<jobId>.jsonl`); stages and the outcome arrive as
    /// `report`s. The work-done `begin` and `end` belong to the caller: the daemon's job layer sends them around
    /// every long-running request (ARCHITECTURE §7.6), so the build sends neither.
    pub async fn build(&self, r: BuildRequest, sink: &dyn EventSink, cancel: CancellationToken) -> Result<BuildOutput> {
        let started = Instant::now();
        let started_at = SystemTime::now();
        let job_id = r.job_id.clone().unwrap_or_else(new_job_id);
        let root = detect::find_root(&r.project)?;
        let log_path = root.join(".russet/logs").join(format!("{job_id}.jsonl"));
        let emitter = Emitter::new(sink, Some(&log_path));
        let result = self.build_inner(&r, &job_id, started, started_at, &emitter, &cancel).await;
        match &result {
            Ok(out) => sink.report(Some(&format!("built {}", out.app.path)), Some(100)),
            Err(e) => sink.report(Some(&format!("build failed: {} ({})", e.message(), e.code())), None),
        }
        emitter.flush();
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_inner(
        &self,
        r: &BuildRequest,
        job_id: &JobId,
        started: Instant,
        started_at: SystemTime,
        em: &Emitter<'_>,
        cancel: &CancellationToken,
    ) -> Result<BuildOutput> {
        // ---- resolve ----------------------------------------------------------------------------------------
        em.stage(Stage::Resolve, StageState::Started);
        let res = match self.resolve(r).await.and_then(|res| check_swift(&res.facts).map(|()| res)) {
            Ok(res) => res,
            Err(e) => {
                em.stage(Stage::Resolve, StageState::Failed);
                return Err(e);
            }
        };
        let work = res.work_dir();
        let _lock = lock_project(&work)?;
        let disk_before = report::dir_size(&res.russet_dir());
        em.stage(Stage::Resolve, StageState::Finished);

        // ---- prepare ----------------------------------------------------------------------------------------
        em.stage(Stage::Prepare, StageState::Started);
        let prepared = self.prepare(r, &res, em, cancel).await;
        let (plan, deployment_target, backend, builder_dir, toolset) = match prepared {
            Ok(p) => p,
            Err(e) => {
                em.stage(Stage::Prepare, StageState::Failed);
                return Err(e);
            }
        };
        em.stage(Stage::Prepare, StageState::Finished);

        // ---- compile ----------------------------------------------------------------------------------------
        em.stage(Stage::Compile, StageState::Started);
        let scratch = work.join("swiftpm");
        let jobs = match res.m().build.jobs {
            manifest::Jobs::Auto => None,
            manifest::Jobs::Count(n) => Some(n),
        };
        let swiftpm = &res.m().build.swiftpm;
        let invocation = spm::Invocation {
            facts: &res.facts,
            backend,
            release: res.profile.release,
            builder_dir: &builder_dir,
            scratch: &scratch,
            cwd: &res.root,
            toolset: &toolset,
            jobs,
            swiftc_flags: &swiftpm.swiftc_flags,
            cc_flags: &swiftpm.cc_flags,
            ld_flags: &swiftpm.ld_flags,
        };
        if let Err(e) = self.compile(&invocation.spec(), em, cancel).await {
            em.stage(Stage::Compile, StageState::Failed);
            return Err(e);
        }
        em.stage(Stage::Compile, StageState::Finished);
        let products_dir = spm::products_dir(&scratch, backend, res.profile.release);

        // ---- resources --------------------------------------------------------------------------------------
        em.stage(Stage::Resources, StageState::Started);
        let icon_png = self.resources(&res, em)?;
        em.stage(Stage::Resources, StageState::Finished);

        // ---- assemble ---------------------------------------------------------------------------------------
        em.stage(Stage::Assemble, StageState::Started);
        let assembled =
            match self.assemble(&res, &plan, deployment_target, &products_dir, icon_png.as_deref(), em, cancel).await {
                Ok(a) => a,
                Err(e) => {
                    em.stage(Stage::Assemble, StageState::Failed);
                    return Err(e);
                }
            };
        let (assembled, dt, mut dsyms) = assembled;
        em.emit(BuildEvent::Artifact { artifact_kind: ArtifactKind::App, path: assembled.app.path.clone() });
        for d in &dsyms {
            em.emit(BuildEvent::Artifact { artifact_kind: ArtifactKind::Dsym, path: d.clone() });
        }
        em.stage(Stage::Assemble, StageState::Finished);

        // ---- lint -------------------------------------------------------------------------------------------
        em.stage(Stage::Lint, StageState::Started);
        let mut lints = assembled.lints.clone();
        lints.extend(self.lint(&res, &assembled.machos));
        let fail = match res.profile.lint {
            manifest::LintMode::Off => false,
            manifest::LintMode::Warn => false,
            manifest::LintMode::Error => lints.iter().any(|l| !l.fixed && l.severity >= LintSeverity::Warning),
            manifest::LintMode::Auto => lints.iter().any(|l| !l.fixed && l.severity == LintSeverity::Error),
        };
        for l in lints.iter().filter(|l| !l.fixed) {
            em.warn(warn::LINT, format!("{} {}", l.code, l.message), None);
        }
        if fail {
            em.stage(Stage::Lint, StageState::Failed);
            return Err(Error::new(
                codes::build::LINT_FAILED,
                format!("{} lint finding(s) at error severity", lints.iter().filter(|l| !l.fixed).count()),
            )
            .with_hint("Fix the findings in build-report.json, or set `[build] lint = \"warn\"` for this profile.")
            .with_details(serde_json::to_value(&lints).unwrap_or_default()));
        }
        em.stage(Stage::Lint, StageState::Finished);

        // ---- package ----------------------------------------------------------------------------------------
        let out_dir = res.out_dir();
        let mut ipa = None;
        if res.package != PackageKind::None {
            em.stage(Stage::Package, StageState::Started);
            match self.package(&res, &assembled.app.path, &out_dir).await {
                Ok(path) => {
                    em.emit(BuildEvent::Artifact { artifact_kind: ArtifactKind::Ipa, path: path.clone() });
                    ipa = Some(path);
                    em.stage(Stage::Package, StageState::Finished);
                }
                Err(e) => {
                    em.stage(Stage::Package, StageState::Failed);
                    return Err(e);
                }
            }
        }

        // ---- report -----------------------------------------------------------------------------------------
        let report_path = out_dir.join("build-report.json");
        let duration = started.elapsed();
        let report = self.report(
            &res,
            job_id,
            started_at,
            duration,
            disk_before,
            &assembled,
            &dt,
            &lints,
            &dsyms,
            ipa.as_deref(),
            backend,
            &plan.app.product,
        );
        report.write(&report_path)?;
        em.emit(BuildEvent::Artifact { artifact_kind: ArtifactKind::Report, path: report_path.clone() });

        let built = BuiltApp {
            app: assembled.app.path.clone(),
            executable: assembled.app.executable.clone(),
            uuids: assembled.uuids.clone(),
            dsyms: dsyms.clone(),
            sdk_path: res.facts.sdk_path.clone(),
            swift_module_dirs: vec![products_dir.clone()],
            flutter_framework: None,
        };
        dsyms.sort();
        Ok(BuildOutput {
            build_id: job_id.clone(),
            app: assembled.app,
            extensions: assembled.extensions,
            bundle_ids: res.ids.map(),
            signing_plan: assembled.signing_plan,
            dsyms,
            ipa,
            report_path,
            lints,
            duration,
            built,
        })
    }

    /// Runs one `swift package` command for the root package and returns its stdout.
    async fn swift_package(
        &self,
        res: &Resolved,
        args: &[&str],
        em: &Emitter<'_>,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>> {
        let scratch = res.work_dir().join("swiftpm");
        let spec = spm::package_command(&res.facts.tools.swift, &res.package_dir(), Some(&scratch), args);
        self.run_tool(&spec, &format!("swift package {}", args.first().copied().unwrap_or_default()), em, cancel).await
    }

    async fn run_tool(
        &self,
        spec: &CommandSpec,
        title: &str,
        em: &Emitter<'_>,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>> {
        // A stable id per tool step: `swift-package-resolve`, `dsymutil`, …
        let task_id: String = title
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let t0 = Instant::now();
        em.emit(BuildEvent::Task {
            task_id: task_id.clone(),
            title: title.to_owned(),
            tool: Some("swift".into()),
            state: TaskState::Started,
            duration_ms: None,
        });
        let lines = spm::LineSink::new(em);
        let out = self.env.runner.run(spec, &lines, cancel).await;
        let ms = Some(t0.elapsed().as_millis() as u64);
        let state = if out.as_ref().is_ok_and(spm::succeeded) { TaskState::Finished } else { TaskState::Failed };
        em.emit(BuildEvent::Task {
            task_id,
            title: title.to_owned(),
            tool: Some("swift".into()),
            state,
            duration_ms: ms,
        });
        let out = out?;
        if !spm::succeeded(&out) {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let last = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output").to_owned();
            return Err(Error::new(
                codes::build::COMPILE_FAILED,
                format!("`{title}` failed: {}", spm::redact_env_dump(&last)),
            )
            .with_hint("Check Package.swift and the package's dependencies (`swift package describe` must work).")
            .with_details(serde_json::json!({"exitCode": out.code, "signal": out.signal})));
        }
        Ok(out.stdout)
    }

    async fn prepare(
        &self,
        r: &BuildRequest,
        res: &Resolved,
        em: &Emitter<'_>,
        cancel: &CancellationToken,
    ) -> Result<(spm::SwiftpmPlan, Version, Backend, Utf8PathBuf, Utf8PathBuf)> {
        let work = res.work_dir();
        let scratch = work.join("swiftpm");
        if r.clean {
            for d in [scratch.clone(), res.out_dir()] {
                if d.exists() {
                    std::fs::remove_dir_all(&d).map_err(|e| spm::io(&d, e))?;
                }
            }
        }
        std::fs::create_dir_all(&scratch).map_err(|e| spm::io(&scratch, e))?;
        self.swift_package(res, &["resolve"], em, cancel).await?;
        let deps_file = work.join("dependencies.json");
        self.swift_package(res, &["show-dependencies", "--format", "json", "-o", deps_file.as_str()], em, cancel)
            .await?;
        let deps_json = std::fs::read(&deps_file).map_err(|e| spm::io(&deps_file, e))?;
        let deps: spm::DependencyNode = serde_json::from_slice(&deps_json).map_err(|e| {
            Error::new(codes::build::COMPILE_FAILED, format!("cannot read {deps_file}: {e}")).with_hint(DEPS_HINT)
        })?;
        let mut dumps = BTreeMap::new();
        let mut stack = vec![&deps];
        while let Some(node) = stack.pop() {
            if dumps.contains_key(&node.identity) {
                continue;
            }
            let stdout = if node.identity == deps.identity {
                self.swift_package(res, &["describe", "--type", "json"], em, cancel).await?
            } else {
                // Its own scratch under .russet/work: without one, SwiftPM writes `.build/` into the dependency's
                // directory, which for a local path dependency is outside the project's `.russet`.
                let scratch = work.join("describe").join(spm::path_component(&node.identity));
                let spec = spm::package_command(
                    &res.facts.tools.swift,
                    Utf8Path::new(&node.path),
                    Some(&scratch),
                    &["describe", "--type", "json"],
                );
                self.run_tool(&spec, &format!("swift package describe ({})", node.name), em, cancel).await?
            };
            dumps.insert(node.identity.clone(), spm::parse_describe(&stdout)?);
            stack.extend(node.dependencies.iter());
        }
        let graph = PackageGraph::new(&deps, dumps)?;
        let m = res.m();
        let wanted = r.target.as_deref().or(m.build.swiftpm.product.as_deref());
        let extensions: Vec<String> = m.app.extensions.iter().map(|e| e.product.clone()).collect();
        let plan = graph.plan(&res.package_dir(), wanted, &extensions)?;
        let dt = deployment_target(m, plan.package_deployment_target, &res.facts)?;
        let mut backend = res.backend;
        match backend {
            Backend::Xtool | Backend::Pif => {
                return Err(Error::new(
                    codes::core::NOT_IMPLEMENTED,
                    format!("the `{}` backend is not available yet", backend.as_str()),
                )
                .with_hint("Use `swiftbuild` (the default) or `native`."));
            }
            Backend::Swiftbuild if plan.has_dynamic_products() => {
                em.warn(
                    warn::BACKEND,
                    "the package graph has .dynamic products, which Swift Build cannot link on Linux; building with the native backend",
                    Some("Track B04 (W16); the native backend is deprecated by SwiftPM 6.4 but links them."),
                );
                backend = Backend::Native;
            }
            _ => {}
        }
        let builder = spm::write_builder(&work, &plan, dt)?;
        let toolset = match res.overrides.get("RUSSET_LINK_SDK_VERSION").map(String::as_str) {
            Some("deployment") => {
                if res.profile.release {
                    return Err(Error::new(
                        codes::config::INVALID,
                        "RUSSET_LINK_SDK_VERSION = \"deployment\" is for debug diagnosis only",
                    )
                    .with_hint("Remove the override from release profiles (ARCHITECTURE §16.4)."));
                }
                em.warn(
                    warn::METADATA,
                    "RUSSET_LINK_SDK_VERSION = deployment: binaries record sdk = minos (diagnosis only)",
                    Some("iOS then applies the deployment target's linked-on-or-after behaviour; never ship this."),
                );
                let kind = if backend == Backend::Native {
                    russet_sdk::toolset::ToolsetKind::Native
                } else {
                    russet_sdk::toolset::ToolsetKind::Swiftbuild
                };
                let t = russet_sdk::toolset::Toolset::new(kind, &res.facts.tools.linker_dir, &res.facts.sdk_path)
                    .without_sdk_fix(&res.facts.sdk_path);
                let path = work.join(format!("toolset-{}-deployment.json", backend.as_str()));
                t.write(&path)?;
                path
            }
            Some(other) => {
                return Err(Error::new(
                    codes::config::INVALID,
                    format!("RUSSET_LINK_SDK_VERSION = \"{other}\" is not supported"),
                )
                .with_hint("The only value is \"deployment\" (ARCHITECTURE §16.4)."));
            }
            None if backend == Backend::Native => res.facts.toolset_native.clone(),
            None => res.facts.toolset_swb.clone(),
        };
        Ok((plan, dt, backend, builder, toolset))
    }

    async fn compile(&self, spec: &CommandSpec, em: &Emitter<'_>, cancel: &CancellationToken) -> Result<()> {
        let t0 = Instant::now();
        em.emit(BuildEvent::Task {
            task_id: "swift-build".into(),
            title: "swift build".into(),
            tool: Some("swift".into()),
            state: TaskState::Started,
            duration_ms: None,
        });
        let lines = spm::LineSink::new(em);
        let out = self.env.runner.run(spec, &lines, cancel).await;
        let ok = out.as_ref().is_ok_and(spm::succeeded);
        em.emit(BuildEvent::Task {
            task_id: "swift-build".into(),
            title: "swift build".into(),
            tool: Some("swift".into()),
            state: if ok { TaskState::Finished } else { TaskState::Failed },
            duration_ms: Some(t0.elapsed().as_millis() as u64),
        });
        out?;
        if !ok {
            if cancel.is_cancelled() {
                return Err(Error::new(codes::core::CANCELLED, "the build was cancelled"));
            }
            return Err(spm::build_failure(&em.diagnostics(), &lines.tail(), em.log_path()));
        }
        Ok(())
    }

    /// Main-bundle resources: a loose PNG icon for development builds; catalogs and string catalogs need
    /// russet-actool and russet-xcstrings (W16, W21).
    fn resources(&self, res: &Resolved, em: &Emitter<'_>) -> Result<Option<Utf8PathBuf>> {
        let m = res.m();
        if !m.app.resources.is_empty() {
            em.warn(
                warn::RESOURCES,
                format!("[app] resources ({}) are not compiled yet", m.app.resources.join(", ")),
                Some("Asset and string catalogs compile with russet-actool and russet-xcstrings (W16, W21)."),
            );
        }
        match &m.app.icon {
            Some(icon) if icon.ends_with(".png") => {
                let path = res.root.join(icon);
                if !path.is_file() {
                    return Err(Error::new(
                        codes::config::INVALID,
                        format!("`[app] icon = \"{icon}\"` does not exist"),
                    )
                    .with_hint("Point `[app] icon` at a PNG in the project, or at an icon set name."));
                }
                Ok(Some(path))
            }
            Some(icon) => {
                em.warn(warn::RESOURCES, format!("the icon set `{icon}` needs an asset catalog compiler (W16)"), None);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    fn build_settings(
        &self,
        res: &Resolved,
        product: &str,
        bundle_id: &str,
        dt: Version,
        version: &str,
        build: &str,
    ) -> SettingsStack {
        let mut s = SettingsStack::new();
        s.push("sdk", metadata::sdk_settings(&res.facts, dt));
        let mut b = BTreeMap::new();
        for (k, v) in [
            ("PRODUCT_NAME", product),
            ("TARGET_NAME", product),
            ("EXECUTABLE_NAME", product),
            ("PRODUCT_BUNDLE_IDENTIFIER", bundle_id),
            ("MARKETING_VERSION", version),
            ("CURRENT_PROJECT_VERSION", build),
            ("DEVELOPMENT_LANGUAGE", "en"),
            ("CONFIGURATION", if res.profile.release { "Release" } else { "Debug" }),
            ("PRODUCT_MODULE_NAME", &russet_plist::macros::c99_identifier(product, true)),
        ] {
            b.insert(k.to_owned(), v.to_owned());
        }
        s.push("build", b);
        s.push("overrides", res.overrides.clone());
        s
    }

    async fn build_number(&self, res: &Resolved, cancel: &CancellationToken) -> Result<String> {
        let raw = res.m().app.build_number.clone().unwrap_or_else(|| "1".to_owned());
        match raw.as_str() {
            "auto" => Ok("1".to_owned()),
            "timestamp" => {
                let ts = events::rfc3339(SystemTime::now());
                Ok(ts[..16].replace(['-', 'T', ':'], ""))
            }
            "git-count" => {
                let git = russet_sdk::host::which("git").ok_or_else(|| {
                    Error::new(codes::toolchain::MISSING, "`build-number = \"git-count\"` needs git on PATH")
                        .with_hint("Install git, or use a literal build number.")
                })?;
                let spec = CommandSpec {
                    program: git.as_std_path().to_owned(),
                    args: vec!["rev-list".into(), "--count".into(), "HEAD".into()],
                    env: spm::tool_env(&[], &[]),
                    env_clear: true,
                    cwd: Some(res.root.as_std_path().to_owned()),
                    nice: None,
                    timeout: Some(Duration::from_secs(30)),
                };
                let out = self.env.runner.run(&spec, &NullSink, cancel).await?;
                let count = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if !spm::succeeded(&out) || count.parse::<u64>().is_err() {
                    return Err(Error::new(
                        codes::config::INVALID,
                        "`build-number = \"git-count\"` needs a git repository with commits",
                    )
                    .with_hint("Commit the project, or use a literal build number."));
                }
                Ok(count)
            }
            literal => Ok(literal.to_owned()),
        }
    }

    /// Validates an entitlements file and writes it as XML under `.russet/work/entitlements/<product>.plist`.
    fn entitlements(&self, res: &Resolved, file: Option<&String>, product: &str) -> Result<Option<Utf8PathBuf>> {
        let Some(file) = file else { return Ok(None) };
        let src = res.root.join(file);
        let d = russet_plist::dictionary_from_path(&src).map_err(|e| {
            Error::new(codes::config::INVALID, format!("{file}: {e}"))
                .with_hint("Entitlements are a property list whose root is a dictionary.")
        })?;
        let out = res.work_dir().join("entitlements").join(format!("{product}.plist"));
        let bytes = russet_plist::to_xml(&Value::Dictionary(d)).map_err(|e| {
            Error::new(codes::config::INVALID, format!("{file}: {e}"))
                .with_hint("Entitlements hold strings, booleans, numbers, arrays and dictionaries only.")
        })?;
        spm::write_if_changed(&out, &bytes)?;
        Ok(Some(out))
    }

    #[allow(clippy::too_many_arguments)]
    async fn assemble(
        &self,
        res: &Resolved,
        plan: &spm::SwiftpmPlan,
        dt_version: Version,
        products_dir: &Utf8Path,
        icon_png: Option<&Utf8Path>,
        em: &Emitter<'_>,
        cancel: &CancellationToken,
    ) -> Result<(bundle::Assembled, Dictionary, Vec<Utf8PathBuf>)> {
        let m = res.m();
        let version = m.app.version.clone().unwrap_or_else(|| "1.0.0".to_owned());
        let build = self.build_number(res, cancel).await?;
        let app_settings = self.build_settings(res, &plan.app.product, &res.ids.app, dt_version, &version, &build);
        let dt = metadata::dt_keys(&res.metadata, &res.facts, &app_settings)?;
        if let MetadataPolicy::XcodeCompat { .. } = res.metadata {
            em.warn(
                warn::METADATA,
                "metadata = xcode-compat: Xcode identity keys are stamped (recorded in build-report.json)",
                None,
            );
        }
        let user_plist = m.app.info_plist.as_ref().map(|p| (res.root.join(p), p.clone()));
        let icon_stem = icon_png.and_then(|p| p.file_stem()).map(str::to_owned);
        let app_info = infoplist::synthesize(&infoplist::InfoPlistInput {
            kind: infoplist::ProductKind::App,
            product: &plan.app.product,
            display_name: m.app.display_name.as_deref(),
            bundle_id: &res.ids.app,
            version: &version,
            build_number: &build,
            deployment_target: dt_version,
            devices: &m.app.devices,
            orientations: &m.app.orientations,
            launch_screen: m.app.launch_screen.as_ref(),
            icon_file: icon_stem.as_deref(),
            user_plist: user_plist.as_ref().map(|(p, l)| (p.as_path(), l.as_str())),
            dt: &dt,
            settings: &app_settings,
        })?;
        for key in &app_info.overridden {
            em.warn(
                warn::INFO_PLIST,
                format!("Info.plist key {key} is set by Russet; the project's value was replaced"),
                None,
            );
        }
        // The app's effective versions (its own Info.plist may set them); extensions get the same (L010).
        let (version, build) = effective_versions(&app_info.plist, version, build);
        let app = ProductBundle {
            plan: plan.app.clone(),
            info: app_info.plist,
            entitlements: self.entitlements(res, m.app.entitlements.as_ref(), &plan.app.product)?,
            bundle_id: res.ids.app.clone(),
            original_bundle_id: res.app_bundle_id.clone(),
            version: version.clone(),
            build_number: build.clone(),
        };
        let mut extensions = Vec::new();
        for (ext_plan, ext) in plan.extensions.iter().zip(&m.app.extensions) {
            let effective = res.ids.extension(&ext.product).unwrap_or_default().to_owned();
            let original = res
                .ids
                .extensions
                .iter()
                .find(|(p, _, _)| *p == ext.product)
                .map(|(_, o, _)| o.clone())
                .unwrap_or_default();
            let settings = self.build_settings(res, &ext.product, &effective, dt_version, &version, &build);
            let user = ext.info_plist.as_ref().map(|p| (res.root.join(p), p.clone()));
            let info = infoplist::synthesize(&infoplist::InfoPlistInput {
                kind: infoplist::ProductKind::AppExtension { kind: ext.kind.clone() },
                product: &ext.product,
                display_name: None,
                bundle_id: &effective,
                version: &version,
                build_number: &build,
                deployment_target: dt_version,
                devices: &m.app.devices,
                orientations: &BTreeMap::new(),
                launch_screen: None,
                icon_file: None,
                user_plist: user.as_ref().map(|(p, l)| (p.as_path(), l.as_str())),
                dt: &dt,
                settings: &settings,
            })?;
            for key in &info.overridden {
                em.warn(
                    warn::INFO_PLIST,
                    format!(
                        "Info.plist key {key} of the extension {} is set by Russet; the project's value was replaced",
                        ext.product
                    ),
                    Some("An extension's versions are its app's, and Russet sets its identity keys (build §4.6)."),
                );
            }
            extensions.push(ProductBundle {
                plan: ext_plan.clone(),
                info: info.plist,
                entitlements: self.entitlements(res, ext.entitlements.as_ref(), &ext.product)?,
                bundle_id: effective,
                original_bundle_id: original,
                version: version.clone(),
                build_number: build.clone(),
            });
        }
        let out_dir = res.out_dir();
        std::fs::create_dir_all(&out_dir).map_err(|e| spm::io(&out_dir, e))?;
        let privacy = m.app.privacy_manifest.as_ref().map(|p| res.root.join(p));
        let input = AssemblyInput {
            products_dir,
            out_dir: &out_dir,
            app: &app,
            extensions: &extensions,
            privacy_manifest: privacy.as_deref(),
            icon_png,
            dt: &dt,
            deployment_target: dt_version,
            back_deploy_dirs: &res.facts.back_deploy_dirs,
            back_deploy_not_needed_from: &res.facts.back_deploy_not_needed_from,
            debug: !res.profile.release,
            epoch: self.env.epoch,
            macho: self.env.macho.as_ref(),
        };
        let assembled = bundle::assemble(&input, em)?;
        let dsyms = self.dsym_and_strip(res, plan, products_dir, &assembled, em, cancel).await?;
        Ok((assembled, dt, dsyms))
    }

    /// Copies or creates the app's dSYM (`[build] dsym`) and strips the executable (`[build] strip`, LLVM ≥ 23).
    async fn dsym_and_strip(
        &self,
        res: &Resolved,
        plan: &spm::SwiftpmPlan,
        products_dir: &Utf8Path,
        assembled: &bundle::Assembled,
        em: &Emitter<'_>,
        cancel: &CancellationToken,
    ) -> Result<Vec<Utf8PathBuf>> {
        let mut dsyms = Vec::new();
        let release = res.profile.release;
        if when(res.profile.dsym, release) {
            let out = assembled
                .app
                .path
                .with_file_name(format!("{}.dSYM", assembled.app.path.file_name().unwrap_or_default()));
            let built = products_dir.join(format!("{}.dSYM", plan.app.target_name));
            if out.exists() {
                std::fs::remove_dir_all(&out).map_err(|e| spm::io(&out, e))?;
            }
            if built.is_dir() {
                bundle::copy_tree(&built, &out)?;
                // Swift Build names the DWARF file after the builder's target; the app's executable is the product.
                let dwarf = out.join("Contents/Resources/DWARF");
                let from = dwarf.join(&plan.app.target_name);
                if from.is_file() {
                    std::fs::rename(&from, dwarf.join(&plan.app.product)).map_err(|e| spm::io(&from, e))?;
                }
                dsyms.push(out);
            } else if let Some(dsymutil) = &res.facts.tools.dsymutil {
                let spec = CommandSpec {
                    program: dsymutil.as_std_path().to_owned(),
                    args: vec![assembled.app.executable.as_str().into(), "-o".into(), out.as_str().into()],
                    env: spm::tool_env(&[&res.facts.tools.linker_dir], &[]),
                    env_clear: true,
                    cwd: Some(res.root.as_std_path().to_owned()),
                    nice: Some(10),
                    timeout: None,
                };
                self.run_tool(&spec, "dsymutil", em, cancel).await?;
                dsyms.push(out);
            } else {
                em.warn(warn::DSYM, "no dsymutil in the SDK bundle; the build has no dSYM", None);
            }
        }
        if when(res.profile.strip, release) {
            match &res.facts.tools.llvm_strip {
                Some(strip) => {
                    let spec = CommandSpec {
                        program: strip.as_std_path().to_owned(),
                        args: vec!["-x".into(), assembled.app.executable.as_str().into()],
                        env: spm::tool_env(&[], &[]),
                        env_clear: true,
                        cwd: None,
                        nice: Some(10),
                        timeout: Some(Duration::from_secs(120)),
                    };
                    self.run_tool(&spec, "llvm-strip", em, cancel).await?;
                    bundle::set_times(&assembled.app.path, self.env.epoch)?;
                }
                None => em.warn(
                    warn::STRIP,
                    "no llvm-strip from LLVM 23 or newer; the executable is not stripped",
                    Some("Install LLVM ≥ 23 (older rewriters can break iOS 27's LINKEDIT alignment rule)."),
                ),
            }
        }
        Ok(dsyms)
    }

    /// L001 and L002 on every Mach-O Russet built: platform iOS, and the SDK version recorded at link time.
    fn lint(&self, res: &Resolved, machos: &[bundle::MachOEntry]) -> Vec<LintFinding> {
        let mut out = Vec::new();
        let sdk_fix_off = res.overrides.get("RUSSET_LINK_SDK_VERSION").is_some_and(|v| v == "deployment");
        for m in machos.iter().filter(|m| m.built_by_russet) {
            if m.summary.platform.is_some_and(|p| p != macho::PLATFORM_IOS) {
                out.push(LintFinding {
                    code: "L001".into(),
                    severity: LintSeverity::Error,
                    path: Some(m.path.clone().into()),
                    message: "LC_BUILD_VERSION platform is not iOS".into(),
                    fixed: false,
                });
            }
            if !sdk_fix_off && m.summary.sdk.is_some_and(|sdk| !sdk.same_minor(res.facts.sdk_version)) {
                let severity = if res.profile.release { LintSeverity::Error } else { LintSeverity::Warning };
                out.push(LintFinding {
                    code: "L002".into(),
                    severity,
                    path: Some(m.path.clone().into()),
                    message: format!(
                        "LC_BUILD_VERSION sdk is {}, not the SDK's {}",
                        m.summary.sdk.map(|v| v.to_string()).unwrap_or_default(),
                        res.facts.sdk_version
                    ),
                    fixed: false,
                });
            }
        }
        out
    }

    async fn package(&self, res: &Resolved, app: &Utf8Path, out_dir: &Utf8Path) -> Result<Utf8PathBuf> {
        let name = app.file_stem().unwrap_or("App");
        let out = out_dir.join(format!("{name}.ipa"));
        let mode =
            if res.package == PackageKind::IpaStore { russet_ipa::IpaMode::Store } else { russet_ipa::IpaMode::Dev };
        let swift_support = if mode == russet_ipa::IpaMode::Store {
            let frameworks = app.join("Frameworks");
            let mut libs: Vec<Utf8PathBuf> = frameworks
                .read_dir_utf8()
                .map(|it| {
                    it.filter_map(|e| e.ok())
                        .map(|e| e.path().to_owned())
                        .filter(|p| p.file_name().is_some_and(|n| n.starts_with("libswift") && n.ends_with(".dylib")))
                        .collect()
                })
                .unwrap_or_default();
            libs.sort();
            let originals: Vec<Utf8PathBuf> = libs
                .iter()
                .filter_map(|l| {
                    res.facts
                        .back_deploy_dirs
                        .iter()
                        .map(|d| d.join(l.file_name().unwrap_or_default()))
                        .find(|p| p.is_file())
                })
                .collect();
            (!originals.is_empty()).then_some(originals)
        } else {
            None
        };
        let opts = russet_ipa::IpaOptions { mode, swift_support, epoch: self.env.epoch };
        let (app, out2) = (app.to_owned(), out.clone());
        let written = tokio::task::spawn_blocking(move || russet_ipa::write(&app, &out2, opts))
            .await
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("IPA packaging panicked: {e}")))?;
        written.map_err(|e| {
            Error::new(codes::build::PACKAGE_FAILED, format!("cannot write {out}: {e}"))
                .with_hint("Check free disk space and the bundle's files.")
                .caused_by(e)
        })?;
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn report(
        &self,
        res: &Resolved,
        job_id: &JobId,
        started_at: SystemTime,
        duration: Duration,
        disk_before: u64,
        assembled: &bundle::Assembled,
        dt: &Dictionary,
        lints: &[LintFinding],
        dsyms: &[Utf8PathBuf],
        ipa: Option<&Utf8Path>,
        backend: Backend,
        product: &str,
    ) -> report::BuildReport {
        let root = &res.root;
        let facts = &res.facts;
        let linker = match &facts.darwin_tools {
            Some(dt) => format!("ld64.lld (darwin-tools {dt})"),
            None => "ld64.lld".to_owned(),
        };
        let bundles = std::iter::once(&assembled.app)
            .chain(&assembled.extensions)
            .map(|b| {
                let prefix =
                    report::rel(&assembled.app.path.parent().map(Utf8Path::to_owned).unwrap_or_default(), &b.path);
                let mach_o = assembled
                    .machos
                    .iter()
                    .filter(|m| m.path.starts_with(&format!("{prefix}/")))
                    .filter(|m| b.kind == BundleKind::AppExtension || !m.path.contains("/PlugIns/"))
                    .map(|m| report::ReportMachO::in_bundle(m, &prefix))
                    .collect();
                report::ReportBundle {
                    path: prefix,
                    kind: match b.kind {
                        BundleKind::App => "app".into(),
                        BundleKind::AppExtension => "app-extension".into(),
                    },
                    bundle_id: b.bundle_id.clone(),
                    original_bundle_id: b.original_bundle_id.clone(),
                    mach_o,
                }
            })
            .collect();
        let disk_after = report::dir_size(&res.russet_dir());
        report::BuildReport {
            schema: 1,
            build_id: job_id.to_string(),
            russet_version: env!("CARGO_PKG_VERSION").to_owned(),
            project: report::ReportProject {
                dir: root.clone(),
                stack: res.stack.as_str().into(),
                product: product.to_owned(),
                backend: backend.as_str().into(),
            },
            profile: res.profile.name.clone(),
            started_at: events::rfc3339(started_at),
            duration_ms: duration.as_millis() as u64,
            disk_delta_bytes: disk_after as i64 - disk_before as i64,
            toolchain: report::ReportToolchain {
                swift: facts.tools.swift_version_text.clone(),
                linker,
                sdk: report::ReportSdk {
                    name: facts.sdk_canonical_name.clone(),
                    build: facts.sdk_build.clone(),
                    xcode: format!("{} ({})", facts.xcode_version, facts.xcode_build),
                    id: facts.id.clone(),
                },
            },
            metadata: report::ReportMetadata {
                policy: res.metadata.as_str().into(),
                stamped: metadata::as_strings(dt),
            },
            bundles,
            lints: lints.to_vec(),
            artifacts: report::ReportArtifacts {
                app: report::rel(root, &assembled.app.path),
                dsym: dsyms.first().map(|d| report::rel(root, d)),
                ipa: ipa.map(|i| report::rel(root, i)),
            },
            signing_plan: report::signing_items(root, &assembled.signing_plan),
        }
    }

    /// The Info.plist a build would write for `product` (the app when omitted), without building
    /// (`project.infoPlist`). Returns the plist and each key's provenance.
    ///
    /// Without `[app] deployment-target`, `MinimumOSVersion` comes from a scan of `Package.swift`'s `.iOS(…)`
    /// platform (builds read it from `swift package describe`), else the default 15.0. An extension gets the app's
    /// effective versions, as in a build.
    pub async fn info_plist_with_provenance(
        &self,
        r: &BuildRequest,
        product: Option<&str>,
    ) -> Result<(Dictionary, BTreeMap<String, String>)> {
        let res = self.resolve(r).await?;
        let m = res.m();
        let package_swift = res.package_dir().join("Package.swift");
        let app_product = r
            .target
            .clone()
            .or_else(|| m.build.swiftpm.product.clone())
            .or_else(|| match detect::swiftpm_products(&package_swift).as_slice() {
                [one] => Some(one.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                Error::new(codes::config::INVALID, "cannot tell the app's product without building")
                    .with_hint("Set `[build.swiftpm] product` in russet.toml.")
            })?;
        let dt_version = deployment_target(m, detect::swiftpm_ios_platform(&package_swift), &res.facts)?;
        let version = m.app.version.clone().unwrap_or_else(|| "1.0.0".to_owned());
        let build = self.build_number(&res, &CancellationToken::new()).await?;
        let wanted = product.unwrap_or(&app_product);
        let ext = m.app.extensions.iter().find(|e| e.product == wanted);
        if ext.is_none() && wanted != app_product {
            return Err(Error::new(codes::core::NOT_FOUND, format!("the project has no target `{wanted}`"))
                .with_hint("`project.info` lists the targets."));
        }
        let synthesize = |kind: infoplist::ProductKind,
                          product: &str,
                          bundle_id: &str,
                          user: Option<&String>,
                          version: &str,
                          build: &str|
         -> Result<infoplist::Synthesized> {
            let settings = self.build_settings(&res, product, bundle_id, dt_version, version, build);
            let dt = metadata::dt_keys(&res.metadata, &res.facts, &settings)?;
            let user = user.map(|p| (res.root.join(p), p.clone()));
            let app = kind == infoplist::ProductKind::App;
            infoplist::synthesize(&infoplist::InfoPlistInput {
                kind,
                product,
                display_name: if app { m.app.display_name.as_deref() } else { None },
                bundle_id,
                version,
                build_number: build,
                deployment_target: dt_version,
                devices: &m.app.devices,
                orientations: &m.app.orientations,
                launch_screen: m.app.launch_screen.as_ref(),
                icon_file: m
                    .app
                    .icon
                    .as_deref()
                    .filter(|i| app && i.ends_with(".png"))
                    .and_then(|i| Utf8Path::new(i).file_stem()),
                user_plist: user.as_ref().map(|(p, l)| (p.as_path(), l.as_str())),
                dt: &dt,
                settings: &settings,
            })
        };
        let app = synthesize(
            infoplist::ProductKind::App,
            &app_product,
            &res.ids.app,
            m.app.info_plist.as_ref(),
            &version,
            &build,
        )?;
        let Some(e) = ext else { return Ok((app.plist, app.provenance)) };
        let (version, build) = effective_versions(&app.plist, version, build);
        let s = synthesize(
            infoplist::ProductKind::AppExtension { kind: e.kind.clone() },
            &e.product,
            res.ids.extension(&e.product).unwrap_or_default(),
            e.info_plist.as_ref(),
            &version,
            &build,
        )?;
        Ok((s.plist, s.provenance))
    }

    /// The Info.plist a build would write (ARCHITECTURE §5.3; the IDE's Info.plist editor preview).
    pub async fn resolve_info_plist(&self, r: &BuildRequest, product: Option<&str>) -> Result<plist::Dictionary> {
        Ok(self.info_plist_with_provenance(r, product).await?.0)
    }

    /// Removes `.russet/work` and `.russet/build` (`Build`) or all of `.russet` (`All`).
    ///
    /// A running build holds `.russet/work/build.lock`, so cleaning then fails with `build.inProgress` instead of
    /// deleting the build's scratch and outputs. The lock is held until everything is removed (the lock file goes
    /// with `.russet/work`; Linux allows removing a locked file), so no build can start halfway through.
    pub async fn clean(&self, dir: &Utf8Path, scope: CleanScope) -> Result<CleanReport> {
        let root = detect::find_root(dir)?;
        let russet = root.join(".russet");
        if !russet.exists() {
            return Ok(CleanReport::default());
        }
        let lock = lock_project(&russet.join("work"))?;
        let targets = match scope {
            CleanScope::Build => vec![russet.join("work"), russet.join("build")],
            CleanScope::All => vec![russet],
        };
        let mut freed = 0;
        for t in targets {
            if t.exists() {
                freed += report::dir_size(&t);
                std::fs::remove_dir_all(&t).map_err(|e| spm::io(&t, e))?;
            }
        }
        drop(lock);
        Ok(CleanReport { freed_bytes: freed })
    }

    /// Inspects an `.app` or `.ipa` (`build.inspect`).
    pub fn inspect(&self, path: &Utf8Path) -> Result<inspect::InspectReport> {
        inspect::inspect(path, self.env.macho.as_ref())
    }
}

/// What [`package_ipa`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagedIpa {
    pub path: Utf8PathBuf,
    pub entries: usize,
    pub bytes: u64,
    pub sha256: String,
}

/// Packs an existing `.app` into an IPA (`russet package ipa`; build §4.8): `store` deflates and is meant for
/// uploads, otherwise entries are stored. `build.packageFailed` (4007) on failure.
pub fn package_ipa(app: &Utf8Path, out: &Utf8Path, store: bool) -> Result<PackagedIpa> {
    let mode = if store { russet_ipa::IpaMode::Store } else { russet_ipa::IpaMode::Dev };
    let r = russet_ipa::write(app, out, russet_ipa::IpaOptions::new(mode)).map_err(|e| {
        Error::new(codes::build::PACKAGE_FAILED, format!("cannot package {app}: {e}"))
            .with_hint("Pass an .app directory and a writable output path.")
            .caused_by(e)
    })?;
    Ok(PackagedIpa { path: r.path, entries: r.entries, bytes: r.bytes, sha256: r.sha256 })
}

/// Build doctor checks, for russetd's doctor seam. The SDK and toolchain checks live in russet-sdk
/// (`sdk.buildFacts`) and W3's toolchain checks; the build plane adds none of its own yet.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn job_ids_are_prefixed_ulids() {
        let a = new_job_id();
        let b = new_job_id();
        assert_ne!(a, b);
        let s = a.as_str();
        assert!(s.starts_with("job_") && s.len() == 30, "{s}");
        assert!(s[4..].chars().all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)));
    }

    #[test]
    fn lint_findings_serialize_with_the_report_names() {
        let l = LintFinding {
            code: "L006".into(),
            severity: LintSeverity::Warning,
            path: Some("A.app/A".into()),
            message: "m".into(),
            fixed: true,
        };
        assert_eq!(
            serde_json::to_value(&l).unwrap_or_default(),
            serde_json::json!({"id": "L006", "severity": "warning", "path": "A.app/A", "message": "m", "fixed": true})
        );
    }

    #[test]
    fn profiles_merge_their_overrides() {
        let mut m = ProjectManifest::default();
        m.build.overrides.insert("A".into(), "1".into());
        m.build.profiles.insert(
            "staging".into(),
            manifest::BuildProfile {
                backend: Some(Backend::Native),
                overrides: BTreeMap::from([("B".to_owned(), "2".to_owned())]),
                ..Default::default()
            },
        );
        let p = profile(&m, Some("staging")).unwrap_or_else(|e| panic!("{e}"));
        assert!(p.release);
        assert_eq!(p.backend, Backend::Native);
        assert_eq!(p.overrides.len(), 2);
        assert!(!profile(&m, None).unwrap_or_else(|e| panic!("{e}")).release);
        assert_eq!(profile(&m, Some("nope")).unwrap_err().code(), codes::config::INVALID);
    }
}
