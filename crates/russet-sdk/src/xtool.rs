// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`XtoolBundleProvider`]: xtool's Swift SDK bundles (`~/.swiftpm/swift-sdks/*.artifactbundle`), read in place
//! and never copied (ARCHITECTURE §15, §16.1). Russet's views and toolsets for them live in `data/sdk/<id>/`.

use std::sync::Arc;

use async_trait::async_trait;
use camino::Utf8PathBuf;
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::proc::CommandRunner;
use russet_doctor::{Category, Check, CheckCx, CheckResult, Requires, Status};

use crate::facts::BundleFacts;
use crate::host::HostToolchain;
use crate::toolset::{Toolset, ToolsetKind};
use crate::{SdkFacts, SdkProvider, SdkSelector, ToolPaths, view};

/// Reads `~/.swiftpm/swift-sdks/*.artifactbundle` in place; never copies (Linux).
#[derive(Debug, Clone)]
pub struct XtoolBundleProvider {
    data_dir: Utf8PathBuf,
    search: Vec<Utf8PathBuf>,
    host: HostToolchain,
    tool: Option<Utf8PathBuf>,
}

impl XtoolBundleProvider {
    /// A provider that writes views and toolsets under `data_dir/sdk/<id>/` (Russet's data directory) and builds
    /// with `host`.
    pub fn new(data_dir: impl Into<Utf8PathBuf>, host: HostToolchain) -> Self {
        Self { data_dir: data_dir.into(), search: default_search_dirs(), host, tool: None }
    }

    /// Searches `dirs` instead of SwiftPM's default `swift-sdks` directories.
    pub fn with_search_dirs(mut self, dirs: Vec<Utf8PathBuf>) -> Self {
        self.search = dirs;
        self
    }

    /// Links `russet-tool` into the view's `usr/local/bin` as `actool`, `ibtool`, … (W16 ships the tool).
    pub fn with_tool(mut self, russet_tool: Option<Utf8PathBuf>) -> Self {
        self.tool = russet_tool;
        self
    }

    /// The host toolchain this provider reports in [`SdkFacts::tools`].
    pub fn host(&self) -> &HostToolchain {
        &self.host
    }

    /// Candidate bundles in the search directories, sorted by path.
    pub fn bundles(&self) -> Vec<Utf8PathBuf> {
        candidate_bundles(&self.search)
    }

    /// Picks the bundle that satisfies `sel`, read-only (no view, no toolsets).
    pub fn inspect(&self, sel: &SdkSelector) -> Result<BundleFacts> {
        select(&self.bundles(), sel)
    }
}

/// `$XDG_CONFIG_HOME/swiftpm/swift-sdks` when `XDG_CONFIG_HOME` is set, else `~/.swiftpm/swift-sdks` (where
/// SwiftPM and xtool keep Swift SDK bundles).
pub fn default_search_dirs() -> Vec<Utf8PathBuf> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if let Some(xdg) = env("XDG_CONFIG_HOME") {
        return vec![Utf8PathBuf::from(xdg).join("swiftpm/swift-sdks")];
    }
    env("HOME")
        .or_else(|| env("USERPROFILE"))
        .map(|h| vec![Utf8PathBuf::from(h).join(".swiftpm/swift-sdks")])
        .unwrap_or_default()
}

fn candidate_bundles(search: &[Utf8PathBuf]) -> Vec<Utf8PathBuf> {
    let mut out: Vec<Utf8PathBuf> = search
        .iter()
        .filter_map(|d| d.read_dir_utf8().ok())
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path().to_owned())
        .filter(|p| p.extension() == Some("artifactbundle") && p.is_dir())
        .collect();
    out.sort();
    out
}

fn matches(sel: &SdkSelector, f: &BundleFacts) -> bool {
    match sel {
        SdkSelector::Auto => true,
        SdkSelector::CanonicalName(n) => f.sdk_canonical_name == *n,
        SdkSelector::XcodeBuild(b) => f.xcode_build.eq_ignore_ascii_case(b),
    }
}

/// The newest bundle (SDK version, then Xcode version and build) that satisfies `sel`.
fn select(bundles: &[Utf8PathBuf], sel: &SdkSelector) -> Result<BundleFacts> {
    let mut found: Vec<BundleFacts> = Vec::new();
    let mut broken: Option<Error> = None;
    for b in bundles {
        if !b.join("Developer/Platforms/iPhoneOS.platform").is_dir() {
            continue; // not a Darwin SDK bundle
        }
        match BundleFacts::read(b) {
            Ok(f) if matches(sel, &f) => found.push(f),
            Ok(_) => {}
            Err(e) => broken = broken.or(Some(e)),
        }
    }
    found.sort_by(|a, b| {
        (a.sdk_version, a.xcode_version, &a.xcode_build).cmp(&(b.sdk_version, b.xcode_version, &b.xcode_build))
    });
    if let Some(best) = found.pop() {
        return Ok(best);
    }
    if let Some(e) = broken {
        return Err(e);
    }
    let what = if *sel == SdkSelector::Auto { "an iOS SDK".to_owned() } else { format!("the iOS SDK `{sel}`") };
    Err(Error::new(codes::sdk::MISSING, format!("no Swift SDK bundle provides {what}"))
        .with_hint("Import one with `russet sdk import <Xcode_27.xip>` (W16b), or run `xtool setup` (xtool's bundle is read in place).")
        .with_details(serde_json::json!({"selector": sel.to_string(), "searched": bundles})))
}

#[async_trait]
impl SdkProvider for XtoolBundleProvider {
    async fn resolve(&self, sel: &SdkSelector) -> Result<SdkFacts> {
        let this = self.clone();
        let sel = sel.clone();
        let task = tokio::task::spawn_blocking(move || this.resolve_blocking(&sel));
        task.await.map_err(|e| Error::new(codes::core::INTERNAL, format!("SDK resolution panicked: {e}")))?
    }
}

impl XtoolBundleProvider {
    /// The synchronous body of [`SdkProvider::resolve`]: select, then materialize the view and toolsets.
    pub fn resolve_blocking(&self, sel: &SdkSelector) -> Result<SdkFacts> {
        let f = self.inspect(sel)?;
        let id = f.id();
        let sdk_dir = self.data_dir.join("sdk").join(&id);
        let platforms_view = view::materialize(&sdk_dir, &view::plan(&f, self.tool.as_deref()))?;
        let linker_dir = f.linker_dir.clone().ok_or_else(|| {
            Error::new(
                codes::toolchain::LINKER_UNSUPPORTED,
                format!("the SDK bundle {} has no darwin-tools ld64.lld", f.bundle),
            )
            .with_hint("Use a bundle with darwin-tools 1.1.0 (`xtool setup`), which reads the SDK's arm64e.x1 TBDs.")
        })?;
        let toolset_swb = sdk_dir.join("toolset-swb.json");
        let toolset_native = sdk_dir.join("toolset-native.json");
        Toolset::new(ToolsetKind::Swiftbuild, &linker_dir, &f.sdk_path).write(&toolset_swb)?;
        Toolset::new(ToolsetKind::Native, &linker_dir, &f.sdk_path).write(&toolset_native)?;
        let h = &self.host;
        let dsymutil = Some(linker_dir.join("dsymutil")).filter(|p| p.is_file());
        let tools = ToolPaths {
            swift: h.swift.clone(),
            swiftc: h.swiftc(),
            clang: h.clang(),
            swift_version: h.version,
            swift_version_text: h.version_text.clone(),
            ld64_lld: linker_dir.join("ld64.lld"),
            librarian: linker_dir.join("llvm-lib"),
            linker_dir,
            dsymutil,
            llvm_strip: h.llvm_strip.clone(),
            llvm_lipo: h.llvm_lipo.clone(),
            llvm_otool: h.llvm_otool.clone(),
            llvm_install_name_tool: h.llvm_install_name_tool.clone(),
        };
        Ok(SdkFacts {
            id,
            sdk_path: f.sdk_path,
            sdk_version: f.sdk_version,
            sdk_canonical_name: f.sdk_canonical_name,
            sdk_build: f.sdk_build,
            platform_build: f.platform_build,
            platform_version: f.platform_version,
            xcode_version: f.xcode_version,
            xcode_build: f.xcode_build,
            swift_required: f.swift_required,
            valid_deployment_targets: f.valid_deployment_targets,
            additional_info: f.additional_info,
            back_deploy_dirs: f.back_deploy_dirs,
            platforms_view,
            toolset_swb,
            toolset_native,
            tools,
            bundle: f.bundle,
            default_compiler: f.default_compiler,
            back_deploy_not_needed_from: f.back_deploy_not_needed_from,
            darwin_tools: f.darwin_tools,
        })
    }
}

/// The daemon's SDK resolution order (ARCHITECTURE §16.1): Russet-imported bundles (W16b), then xtool's bundles
/// read in place. The host toolchain is probed through `runner` on each resolve (W3's resolver replaces the
/// probe), so creating the provider costs nothing.
#[derive(Clone)]
pub struct DefaultSdkProvider {
    data_dir: Utf8PathBuf,
    tool: Option<Utf8PathBuf>,
    runner: Arc<dyn CommandRunner>,
}

impl std::fmt::Debug for DefaultSdkProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultSdkProvider")
            .field("data_dir", &self.data_dir)
            .field("tool", &self.tool)
            .finish_non_exhaustive()
    }
}

impl DefaultSdkProvider {
    /// Views and toolsets go under `data_dir/sdk/<id>/`; `russet_tool` is linked into the views when given; the
    /// toolchain probes run through `runner` (the daemon's `ProcessRunner`).
    pub fn new(
        data_dir: impl Into<Utf8PathBuf>,
        russet_tool: Option<Utf8PathBuf>,
        runner: Arc<dyn CommandRunner>,
    ) -> Self {
        Self { data_dir: data_dir.into(), tool: russet_tool, runner }
    }
}

#[async_trait]
impl SdkProvider for DefaultSdkProvider {
    async fn resolve(&self, sel: &SdkSelector) -> Result<SdkFacts> {
        let host = HostToolchain::probe(self.runner.as_ref()).await?;
        XtoolBundleProvider::new(&self.data_dir, host).with_tool(self.tool.clone()).resolve(sel).await
    }
}

/// `sdk.buildFacts`: an iOS SDK bundle that has every file the build plane needs (ARCHITECTURE §16.3). A bundle
/// without the platform `version.plist` (DTPlatformBuild, so every `metadata = "sdk"` build fails) or without
/// darwin-tools' `ld64.lld` (nothing links) fails the check.
#[derive(Debug, Clone, Default)]
pub struct BuildFactsCheck {
    search: Option<Vec<Utf8PathBuf>>,
}

impl BuildFactsCheck {
    /// Looks in SwiftPM's `swift-sdks` directories ([`default_search_dirs`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks in `dirs` instead (tests).
    pub fn with_search_dirs(dirs: Vec<Utf8PathBuf>) -> Self {
        Self { search: Some(dirs) }
    }
}

#[async_trait]
impl Check for BuildFactsCheck {
    fn id(&self) -> &'static str {
        "sdk.buildFacts"
    }

    fn title(&self) -> &'static str {
        "An iOS SDK the build plane can use"
    }

    fn category(&self) -> Category {
        Category::Toolchain
    }

    fn requires(&self) -> Requires {
        Requires::default()
    }

    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        let bundles = candidate_bundles(&self.search.clone().unwrap_or_else(default_search_dirs));
        let picked = tokio::task::spawn_blocking(move || select(&bundles, &SdkSelector::Auto)).await;
        match picked {
            Ok(Ok(f)) => {
                let tools = f.darwin_tools.as_deref().map_or_else(String::new, |t| format!(", darwin-tools {t}"));
                let mut summary =
                    format!("{} ({}, needs Swift {}{tools})", f.id(), f.sdk_canonical_name, f.swift_required);
                let mut missing = Vec::new();
                if f.platform_build.is_none() {
                    missing.push("the iPhoneOS platform version.plist (builds with the default `metadata = \"sdk\"` fail with sdk.incomplete)");
                }
                if f.linker_dir.is_none() {
                    missing.push("darwin-tools' ld64.lld (nothing links: toolchain.linkerUnsupported)");
                }
                let (status, hint) = if missing.is_empty() {
                    (Status::Pass, None)
                } else {
                    summary = format!("{summary} lacks {}", missing.join(" and "));
                    (
                        Status::Fail,
                        Some("Recreate the bundle with `xtool setup` (darwin-tools 1.1.0), or import an Xcode with `russet sdk import`; `[build] metadata = \"minimal\"` builds without the version.plist.".to_owned()),
                    )
                };
                CheckResult { status, summary, detail: Some(f.bundle.to_string()), hint }
            }
            Ok(Err(e)) => CheckResult {
                status: Status::Fail,
                summary: format!("{} ({})", e.message(), e.code()),
                detail: None,
                hint: e.hint().map(str::to_owned),
            },
            Err(e) => CheckResult {
                status: Status::Fail,
                summary: format!("the check panicked: {e}"),
                detail: None,
                hint: None,
            },
        }
    }
}
