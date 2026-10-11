// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Toolchains (ARCHITECTURE §16.1–§16.2; core §5.6, §10): the pin manifest, probes, the resolver with its env
//! contract (`ToolInfo.env` is part of the contract), the installer, GC and the SDK registry probe.
//!
//! - [`pins`]: `pins/stable.toml`, minisign-verified and embedded [D 87].
//! - [`probe`]: Swift, Darwin SDKs, linkers, LLDB and lldb-dap with their libpython env (RD-05), xtool, Flutter.
//! - [`Toolchains::resolve`] and [`Toolchains::resolve_build_env`]: the resolution order and compatibility rules of
//!   §16.1 (`sdk.incompatible`, `toolchain.versionMismatch`, `toolchain.linkerUnsupported`).
//! - [`install`]: verified installs of `python-shared` and `darwin-tools` (Wave 1; no Swift or LLVM downloads).
//!
//! Layer L2 may not depend on russet-doctor (ARCHITECTURE §5.2), so the toolchain checks live in russetd
//! (`handlers/doctor.rs`) [D 100].

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use russet_config::project::ToolchainSection;
use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, codes};
use russet_core::progress::ProgressSink;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub mod exec;
pub mod host;
pub mod install;
pub mod pins;
pub mod probe;
pub mod sdk;
pub mod version;

use crate::exec::{CachedExec, Exec, ProcessExec};
use crate::host::HostEnv;
use crate::pins::PinManifest;
use crate::probe::Prober;
use crate::version::{Version, VersionReq};

/// A toolchain component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Component {
    Swift,
    DarwinSdk,
    Linker,
    Lldb,
    LldbDap,
    LibPython,
    Xtool,
    Flutter,
    GenSnapshotIos,
    Llvm,
}

impl Component {
    /// Every component, in report order.
    pub const ALL: [Component; 10] = [
        Self::Swift,
        Self::DarwinSdk,
        Self::Linker,
        Self::Lldb,
        Self::LldbDap,
        Self::LibPython,
        Self::Xtool,
        Self::Flutter,
        Self::GenSnapshotIos,
        Self::Llvm,
    ];

    /// The wire name (kebab-case).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Swift => "swift",
            Self::DarwinSdk => "darwin-sdk",
            Self::Linker => "linker",
            Self::Lldb => "lldb",
            Self::LldbDap => "lldb-dap",
            Self::LibPython => "libpython",
            Self::Xtool => "xtool",
            Self::Flutter => "flutter",
            Self::GenSnapshotIos => "gen-snapshot-ios",
            Self::Llvm => "llvm",
        }
    }

    /// Parses a wire name or a managed component's pin name (`python-shared` → [`Component::LibPython`],
    /// `darwin-tools` → [`Component::Linker`]).
    pub fn parse(s: &str) -> Result<Self> {
        let alias = match s {
            "python-shared" | "python" => Some(Self::LibPython),
            "darwin-tools" => Some(Self::Linker),
            _ => None,
        };
        alias.or_else(|| Self::ALL.into_iter().find(|c| c.as_str() == s)).ok_or_else(|| {
            Error::new(codes::core::INVALID_ARGUMENT, format!("`{s}` is not a toolchain component")).with_hint(
                "Use swift, darwin-sdk, linker, lldb, lldb-dap, libpython, xtool or flutter; install takes \
                 python-shared or darwin-tools.",
            )
        })
    }

    /// The pin-manifest name of the managed (downloadable) form of this component.
    pub const fn managed_name(self) -> Option<&'static str> {
        match self {
            Self::LibPython => Some("python-shared"),
            Self::Linker => Some("darwin-tools"),
            Self::Swift => Some("swift"),
            Self::Llvm => Some("llvm"),
            Self::Flutter => Some("flutter"),
            Self::Xtool => Some("xtool"),
            _ => None,
        }
    }
}

/// Where a resolved tool came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    Managed {
        pin: String,
    },
    System,
    /// An existing uv-managed python-build-standalone (ARCHITECTURE §16.1).
    Uv,
    XtoolBundle {
        bundle: PathBuf,
    },
    User,
    ProjectPin,
}

/// A resolved tool. `env` is part of the contract: every launcher applies it (ARCHITECTURE §16.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInfo {
    pub component: Component,
    pub version: String,
    pub path: PathBuf,
    pub provenance: Provenance,
    pub env: BTreeMap<String, String>,
    pub ok: bool,
    pub problems: Vec<String>,
}

/// A Darwin SDK known to the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkInfo {
    /// For example `xcode-27.0-27A266a`.
    pub id: String,
    pub xcode_version: String,
    pub xcode_build: String,
    pub ios_sdk: String,
    pub swift_sdk_name: String,
    pub bundle: PathBuf,
    pub darwin_tools: Option<String>,
    pub compatible_swift: String,
}

/// A requirement for [`Toolchains::resolve`] (F2 refinement of F0's `{version}`): a version and, for the linker
/// and LLDB, a flavor or path from the project (`[toolchain] linker`, `[debug] lldb`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Requirement {
    /// `6.4` (any 6.4.x), `=6.4.0` or `>=23.1.3`.
    pub version: Option<String>,
    /// `auto`; for LLDB `swift`, `llvm` or a path; for the linker `darwin-tools@1.1.0`, `llvm@>=23.1.3` or a path.
    pub flavor: Option<String>,
}

impl Requirement {
    /// Anything that works.
    pub fn any() -> Self {
        Self::default()
    }

    /// A version requirement.
    pub fn version(v: impl Into<String>) -> Self {
        Self { version: Some(v.into()), flavor: None }
    }

    /// A flavor or path.
    pub fn flavor(f: impl Into<String>) -> Self {
        Self { version: None, flavor: Some(f.into()) }
    }
}

/// What [`Toolchains::gc`] removed (or would remove).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    pub freed_bytes: u64,
    pub removed: Vec<PathBuf>,
}

/// A consistent Swift + SDK + linker for a build (core §5.6, §10.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildEnv {
    pub swift: ToolInfo,
    pub sdk: SdkInfo,
    pub linker: ToolInfo,
    /// The merged environment of the tools.
    pub env: BTreeMap<String, String>,
}

/// The toolchain manager.
pub struct Toolchains {
    dirs: RussetDirs,
    pins: Arc<PinManifest>,
    pins_source: String,
    host: HostEnv,
    raw: Arc<dyn Exec>,
    exec: Arc<CachedExec>,
    min_free_space: u64,
    offline: bool,
    tx: broadcast::Sender<Vec<Component>>,
    install_lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for Toolchains {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Toolchains")
            .field("dirs", &self.dirs)
            .field("pins", &self.pins.id)
            .field("pins_source", &self.pins_source)
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

fn missing(c: Component, hint: &str) -> Error {
    Error::new(codes::toolchain::MISSING, format!("no {} was found", c.as_str())).with_hint(hint.to_owned())
}

fn missing_hint(c: Component) -> &'static str {
    match c {
        Component::Swift => "Install Swift 6.4 (swift.org, or the AUR package swift-bin) and put `swift` on PATH.",
        Component::DarwinSdk => {
            "Import the Xcode 27 SDK: `russet sdk import <Xcode_27.xip>` (or install xtool's SDK bundle)."
        }
        Component::Linker => "Run `russet toolchain install darwin-tools`.",
        Component::Lldb | Component::LldbDap => "Install Swift 6.4, whose toolchain carries lldb and lldb-dap.",
        Component::LibPython => "Run `russet toolchain install python-shared`.",
        Component::Xtool => "xtool is optional; install it from github.com/xtool-org/xtool if you need it.",
        Component::Flutter => "Install Flutter 3.47.7 and put `flutter` on PATH.",
        Component::GenSnapshotIos | Component::Llvm => "This component is not available in this version of Russet.",
    }
}

impl Toolchains {
    /// The manager for `dirs` with `pins`, probing this host.
    pub fn new(dirs: RussetDirs, pins: Arc<PinManifest>) -> Self {
        let raw: Arc<dyn Exec> = Arc::new(ProcessExec);
        let exec = Arc::new(CachedExec::new(raw.clone(), Some(dirs.cache.join("toolchain-probes.json"))));
        let (tx, _) = broadcast::channel(16);
        Self {
            dirs,
            pins,
            pins_source: "embedded".into(),
            host: HostEnv::from_env(),
            raw,
            exec,
            min_free_space: 5 << 30,
            offline: std::env::var_os("RUSSET_OFFLINE").is_some_and(|v| v == "1"),
            tx,
            install_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// The manager with the embedded, verified pin manifest.
    pub fn load(dirs: &RussetDirs) -> Result<Self> {
        Ok(Self::new(dirs.clone(), Arc::new(PinManifest::embedded()?)))
    }

    /// Probes `host` instead of this process's environment (tests).
    pub fn with_host(mut self, host: HostEnv) -> Self {
        self.host = host;
        self
    }

    /// Runs probes through `exec` (tests); results are cached in memory only.
    pub fn with_exec(mut self, exec: Arc<dyn Exec>) -> Self {
        self.exec = Arc::new(CachedExec::new(exec.clone(), None));
        self.raw = exec;
        self
    }

    /// The free space installs must leave (`storage.min-free-space`, default 5 GiB).
    pub fn with_min_free_space(mut self, bytes: u64) -> Self {
        self.min_free_space = bytes;
        self
    }

    /// Refuses non-loopback downloads (`RUSSET_OFFLINE=1`).
    pub fn with_offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    /// The pin manifest.
    pub fn pins(&self) -> &PinManifest {
        &self.pins
    }

    /// Where the manifest came from (`embedded`).
    pub fn pins_source(&self) -> &str {
        &self.pins_source
    }

    /// Receives `toolchain.changed` (the components an install, uninstall or GC changed).
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<Component>> {
        self.tx.subscribe()
    }

    fn prober(&self) -> Prober<'_> {
        Prober { host: &self.host, exec: &*self.exec, raw: &*self.raw, data_dir: &self.dirs.data, pins: &self.pins }
    }

    fn sdks_now(&self) -> Vec<SdkInfo> {
        let mut out = sdk::registry_sdks(&self.dirs.data, &self.pins);
        for s in sdk::bundle_sdks(&self.host.swift_sdk_dirs, &self.pins) {
            if !out.iter().any(|o| o.id == s.id && o.bundle == s.bundle) {
                out.push(s);
            }
        }
        out
    }

    /// Every candidate of every component, in resolution order, with problems. `refresh` re-runs every probe.
    pub async fn probe_all(&self, refresh: bool) -> Vec<ToolInfo> {
        if refresh {
            self.exec.clear();
        }
        let p = self.prober();
        let swift = p.swift().await;
        let sdks = self.sdks_now();
        let mut out = swift.clone();
        out.extend(p.darwin_sdks(&sdks, &swift));
        out.extend(p.linkers(&sdks).await);
        let roots: Vec<PathBuf> = swift.iter().filter_map(|s| probe::swift_root(&s.path)).collect();
        out.extend(p.swift_lldb(&swift, false).await);
        out.extend(p.llvm_lldb(&roots, false).await);
        out.extend(p.swift_lldb(&swift, true).await);
        out.extend(p.llvm_lldb(&roots, true).await);
        let sonames = roots.first().map(|r| p.liblldb_python_needs(r)).unwrap_or_default();
        let sonames = if sonames.is_empty() { vec!["libpython3.12.so.1.0".to_owned()] } else { sonames };
        for s in &sonames {
            out.extend(p.libpython(s).await);
        }
        out.extend(p.simple(Component::Xtool, "xtool").await);
        out.extend(p.simple(Component::Flutter, "flutter").await);
        out
    }

    /// The Darwin SDKs: Russet's imports (`data/sdks/registry.json`), then xtool bundles read in place.
    pub async fn sdks(&self) -> Result<Vec<SdkInfo>> {
        Ok(self.sdks_now())
    }

    /// The first working candidate of `c` that meets `req` (ARCHITECTURE §16.1 order).
    pub async fn resolve(&self, c: Component, req: &Requirement) -> Result<ToolInfo> {
        let p = self.prober();
        let flavor = req.flavor.as_deref().filter(|f| !f.is_empty() && *f != "auto");
        let candidates: Vec<ToolInfo> = match c {
            Component::Swift => p.swift().await,
            Component::DarwinSdk => {
                let swift = p.swift().await;
                p.darwin_sdks(&self.sdks_now(), &swift)
            }
            Component::Linker => {
                if let Some(path) = flavor.filter(|f| f.contains('/')) {
                    return self.explicit_tool(c, path).await;
                }
                p.linkers(&self.sdks_now()).await
            }
            Component::Lldb | Component::LldbDap => {
                let dap = c == Component::LldbDap;
                if let Some(path) = flavor.filter(|f| f.contains('/')) {
                    return self.explicit_tool(c, path).await;
                }
                let swift = p.swift().await;
                let roots: Vec<PathBuf> = swift.iter().filter_map(|s| probe::swift_root(&s.path)).collect();
                let mut all = Vec::new();
                if flavor != Some("llvm") {
                    all.extend(p.swift_lldb(&swift, dap).await);
                }
                if flavor != Some("swift") {
                    all.extend(p.llvm_lldb(&roots, dap).await);
                }
                all
            }
            Component::LibPython => {
                let swift = p.swift().await;
                let soname = swift
                    .iter()
                    .filter_map(|s| probe::swift_root(&s.path))
                    .flat_map(|r| p.liblldb_python_needs(&r))
                    .next()
                    .unwrap_or_else(|| "libpython3.12.so.1.0".into());
                p.libpython(&soname).await
            }
            Component::Xtool => p.simple(c, "xtool").await,
            Component::Flutter => p.simple(c, "flutter").await,
            Component::GenSnapshotIos | Component::Llvm => Vec::new(),
        };
        self.pick(c, candidates, req, flavor)
    }

    fn pick(
        &self,
        c: Component,
        candidates: Vec<ToolInfo>,
        req: &Requirement,
        flavor: Option<&str>,
    ) -> Result<ToolInfo> {
        if candidates.is_empty() {
            let code_err = if c == Component::DarwinSdk {
                Error::new(codes::sdk::MISSING, "no Darwin SDK was found").with_hint(missing_hint(c))
            } else {
                missing(c, missing_hint(c))
            };
            return Err(code_err);
        }
        let version_req: Option<VersionReq> = req
            .version
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.parse().map_err(|e: String| {
                    Error::new(codes::config::INVALID, format!("invalid version requirement: {e}"))
                        .with_hint("Write a version such as 6.4, =6.4.0 or >=23.1.3.")
                })
            })
            .transpose()?;
        let bundle_darwin_tools: BTreeMap<PathBuf, String> = if c == Component::Linker {
            self.sdks_now().into_iter().filter_map(|s| Some((s.bundle, s.darwin_tools?))).collect()
        } else {
            BTreeMap::new()
        };
        let flavor_ok = |t: &ToolInfo| match (c, flavor) {
            (Component::Linker, Some(f)) => linker_flavor_matches(t, f, &bundle_darwin_tools),
            _ => true,
        };
        let version_ok = |t: &ToolInfo| match &version_req {
            Some(r) => t.version.parse::<Version>().is_ok_and(|v| r.matches(&v)),
            None => true,
        };
        if let Some(t) = candidates.iter().find(|t| t.ok && flavor_ok(t) && version_ok(t)) {
            return Ok(t.clone());
        }
        // Explain the failure with the most specific code.
        let wanted = candidates.iter().filter(|t| flavor_ok(t)).collect::<Vec<_>>();
        if let Some(r) = &version_req
            && wanted.iter().any(|t| t.ok)
        {
            let found: Vec<String> = wanted.iter().filter(|t| t.ok).map(|t| t.version.clone()).collect();
            return Err(Error::new(
                codes::toolchain::VERSION_MISMATCH,
                format!("{} {r} is required; found {}", c.as_str(), found.join(", ")),
            )
            .with_hint(format!("Install {} {r}, or change the project's [toolchain] pin.", c.as_str()))
            .with_details(serde_json::json!({ "component": c.as_str(), "required": r.to_string(), "found": found })));
        }
        let first = wanted.first().copied().or(candidates.first());
        let problems = first.map(|t| t.problems.join("; ")).unwrap_or_default();
        let err = match c {
            Component::Linker => Error::new(
                codes::toolchain::LINKER_UNSUPPORTED,
                format!("no usable linker for the Xcode 27 SDK: {problems}"),
            )
            .with_hint("Run `russet toolchain install darwin-tools`, or install LLD >= 23.1.3."),
            Component::DarwinSdk => Error::new(codes::sdk::INCOMPATIBLE, format!("no usable Darwin SDK: {problems}"))
                .with_hint("Install the Swift version the SDK needs (Xcode 27 → Swift 6.4), or import a matching SDK."),
            Component::Lldb | Component::LldbDap if problems.contains("libpython") => {
                Error::new(codes::toolchain::BROKEN, format!("{} cannot start: {problems}", c.as_str()))
                    .with_hint("Run `russet toolchain install python-shared`.")
            }
            _ if flavor.is_some() && wanted.is_empty() => missing(c, missing_hint(c)),
            _ => Error::new(codes::toolchain::BROKEN, format!("{} does not work: {problems}", c.as_str()))
                .with_hint(missing_hint(c)),
        };
        Err(err.with_details(serde_json::json!({ "component": c.as_str(), "problems": problems })))
    }

    async fn explicit_tool(&self, c: Component, path: &str) -> Result<ToolInfo> {
        let p = PathBuf::from(path);
        if !host::is_executable(&p) {
            return Err(Error::new(codes::toolchain::MISSING, format!("{path} is not an executable"))
                .with_hint("Fix the path in the project file, or use `auto`."));
        }
        let out = self.exec.output(&p, &["--version"], &BTreeMap::new(), std::time::Duration::from_secs(10)).await?;
        let version = Version::find_in(&out.combined()).map(|v| v.to_string()).unwrap_or_default();
        Ok(ToolInfo {
            component: c,
            version,
            path: p,
            provenance: Provenance::ProjectPin,
            env: BTreeMap::new(),
            ok: out.success(),
            problems: if out.success() { Vec::new() } else { vec![format!("`{path} --version` failed")] },
        })
    }

    /// A consistent Swift, SDK and linker for a build (core §10.3): the SDK's Xcode row sets the Swift minor
    /// (`sdk.incompatible` otherwise), a project pin must match (`toolchain.versionMismatch`), and the linker must
    /// be darwin-tools ≥ 1.1.0 or LLD ≥ 23.1.3 (`toolchain.linkerUnsupported`).
    pub async fn resolve_build_env(&self, project: &ToolchainSection) -> Result<BuildEnv> {
        let sdks = self.sdks_now();
        let sdk = match project.xcode_sdk.as_deref().filter(|v| !v.is_empty()) {
            Some(want) => sdks
                .iter()
                .find(|s| s.xcode_version == want || s.ios_sdk == want || s.xcode_build == want || s.id == want)
                .cloned()
                .ok_or_else(|| {
                    Error::new(
                        codes::sdk::MISSING,
                        format!("no imported SDK matches [toolchain] xcode-sdk = \"{want}\""),
                    )
                    .with_hint(format!("Import the Xcode {want} SDK with `russet sdk import`, or change the pin."))
                })?,
            None => sdks
                .iter()
                .max_by(|a, b| {
                    let va = a.xcode_version.parse::<Version>().ok();
                    let vb = b.xcode_version.parse::<Version>().ok();
                    va.cmp(&vb)
                })
                .cloned()
                .ok_or_else(|| {
                    Error::new(codes::sdk::MISSING, "no Darwin SDK was found")
                        .with_hint(missing_hint(Component::DarwinSdk))
                })?,
        };
        let row =
            self.pins.xcode(&sdk.xcode_build).or_else(|| self.pins.xcode(&sdk.xcode_version)).ok_or_else(|| {
                Error::new(
                    codes::sdk::INCOMPATIBLE,
                    format!(
                        "the pin manifest has no compatibility row for Xcode {} ({})",
                        sdk.xcode_version, sdk.xcode_build
                    ),
                )
                .with_hint("Update the pins (`russet toolchain pins update`), or import a supported Xcode SDK.")
            })?;
        let swift =
            self.resolve(Component::Swift, &Requirement { version: project.swift.clone(), flavor: None }).await?;
        let needed: Version = row.swift.parse().map_err(|e: String| {
            Error::new(codes::toolchain::PINS_INVALID, format!("bad swift in the Xcode {} row: {e}", row.version))
                .with_hint("Regenerate the pin manifest.")
        })?;
        let found: Version = swift.version.parse().map_err(|_| {
            Error::new(codes::toolchain::BROKEN, format!("cannot read the version of {}", swift.path.display()))
                .with_hint("Run `swift --version` by hand.")
        })?;
        if !found.matches_prefix(&needed) {
            return Err(Error::new(
                codes::sdk::INCOMPATIBLE,
                format!("the Xcode {} SDK needs Swift {needed}; {} is Swift {found}", sdk.xcode_version, swift.path.display()),
            )
            .with_hint(format!("Install Swift {needed} (\"this SDK is not supported by the compiler\" otherwise)."))
            .with_details(serde_json::json!({ "sdk": sdk.id, "swiftRequired": needed.to_string(), "swiftFound": found.to_string() })));
        }
        let linker =
            self.resolve(Component::Linker, &Requirement { version: None, flavor: project.linker.clone() }).await?;
        let mut env = BTreeMap::new();
        for t in [&swift, &linker] {
            env.extend(t.env.clone());
        }
        Ok(BuildEnv { swift, sdk, linker, env })
    }

    /// Installs a pinned version (`python-shared` or `darwin-tools` in Wave 1) and returns the resolved tool.
    pub async fn install(
        &self,
        c: Component,
        version: &str,
        p: &dyn ProgressSink,
        cancel: &CancellationToken,
    ) -> Result<ToolInfo> {
        let name = match c {
            Component::LibPython | Component::Linker => c.managed_name().unwrap_or_default(),
            other => {
                return Err(Error::new(
                    codes::core::NOT_IMPLEMENTED,
                    format!(
                        "installing {} is not available yet; Wave 1 installs python-shared and darwin-tools",
                        other.as_str()
                    ),
                )
                .with_hint(missing_hint(other)));
            }
        };
        let wanted = if version.is_empty() || version == "latest" { None } else { Some(version) };
        let pin = self.pins.component(name, wanted).ok_or_else(|| {
            Error::new(
                codes::toolchain::MISSING,
                format!("the pin manifest has no {name} {}", wanted.unwrap_or("release")),
            )
            .with_hint("`russet toolchain list` shows the pinned versions.")
        })?;
        let _guard = self.install_lock.lock().await;
        let job = install::InstallJob {
            data_dir: &self.dirs.data,
            cache_dir: &self.dirs.cache,
            min_free_space: self.min_free_space,
            offline: self.offline,
            progress: p,
            cancel,
        };
        let dir = job.run(pin, &pins::host_platform()).await?;
        let _ = self.tx.send(vec![c]);
        let probe = self.prober();
        let tool = match c {
            Component::LibPython => {
                let soname = format!("libpython{}.so.1.0", minor_of(&pin.version));
                probe.libpython(&soname).await.into_iter().find(|t| t.path.starts_with(&dir))
            }
            _ => probe.linkers(&[]).await.into_iter().find(|t| t.path.starts_with(&dir)),
        };
        tool.ok_or_else(|| {
            Error::new(
                codes::toolchain::BROKEN,
                format!("{name} {} installed in {} but does not resolve", pin.version, dir.display()),
            )
            .with_hint("Remove it with `russet toolchain uninstall` and install it again.")
        })
    }

    /// Removes an installed version; returns the bytes freed.
    pub async fn uninstall(&self, c: Component, version: &str) -> Result<u64> {
        let name = c.managed_name().ok_or_else(|| {
            Error::new(codes::core::INVALID_ARGUMENT, format!("{} has no managed installs", c.as_str()))
                .with_hint("Uninstall python-shared or darwin-tools.")
        })?;
        let _guard = self.install_lock.lock().await;
        let freed = install::uninstall(&self.dirs.data, name, version)?;
        let _ = self.tx.send(vec![c]);
        Ok(freed)
    }

    /// Removes installed versions that are not selected, and leftovers of interrupted installs.
    pub async fn gc(&self, dry_run: bool) -> Result<GcReport> {
        let _guard = self.install_lock.lock().await;
        let mut report = GcReport::default();
        let mut changed = Vec::new();
        for (name, _version, dir, bytes) in install::unselected(&self.dirs.data) {
            report.freed_bytes += bytes;
            if !dry_run {
                std::fs::remove_dir_all(&dir).map_err(|e| {
                    Error::new(codes::core::IO, format!("cannot remove {}: {e}", dir.display()))
                        .with_hint("Check the permissions of Russet's data directory.")
                })?;
                if let Ok(c) = Component::parse(&name) {
                    changed.push(c);
                }
            }
            report.removed.push(dir);
        }
        for dir in install::stale_staging(&self.dirs.data) {
            report.freed_bytes += install::dir_size(&dir);
            if !dry_run {
                let _ = std::fs::remove_dir_all(&dir);
            }
            report.removed.push(dir);
        }
        if !changed.is_empty() {
            let _ = self.tx.send(changed);
        }
        Ok(report)
    }
}

/// Whether a linker candidate matches `darwin-tools@<req>`, `llvm@<req>` or `lld@<req>`; `bundles` maps an SDK
/// bundle to the darwin-tools release it carries.
fn linker_flavor_matches(t: &ToolInfo, flavor: &str, bundles: &BTreeMap<PathBuf, String>) -> bool {
    let (kind, req) = flavor.split_once('@').unwrap_or((flavor, ""));
    let req: Option<VersionReq> = (!req.is_empty()).then(|| req.parse().ok()).flatten();
    match kind {
        "darwin-tools" => {
            let dt_version = match &t.provenance {
                Provenance::Managed { pin } => pin.strip_prefix("darwin-tools@").map(str::to_owned),
                Provenance::XtoolBundle { bundle } => bundles.get(bundle).cloned(),
                _ => None,
            };
            match (dt_version, &req) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(v), Some(r)) => v.parse::<Version>().is_ok_and(|v| r.matches(&v)),
            }
        }
        "llvm" | "lld" => {
            t.provenance == Provenance::System
                && req.as_ref().is_none_or(|r| t.version.parse::<Version>().is_ok_and(|v| r.matches(&v)))
        }
        _ => false,
    }
}

fn minor_of(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}
