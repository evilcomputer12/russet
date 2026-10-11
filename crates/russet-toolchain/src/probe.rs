// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Probes (core §10.1; RD §8): every candidate of each component, in the resolution order of ARCHITECTURE
//! §16.1, with its version, provenance, environment and problems. The resolver picks the first `ok` candidate.
//!
//! LLDB (RD-05): flavor `swift` (the build toolchain's LLDB 21, default) or `llvm` (a distro LLDB ≥ 23.1). Swift's
//! Linux `liblldb` has a hard `DT_NEEDED` on `libpython3.12.so.1.0`, so the resolver reads that soname with goblin
//! and finds a matching libpython: the `ldconfig` cache, an existing uv-managed python-build-standalone (provenance
//! `uv`), or Russet's managed `python-shared`. The env contract (ARCHITECTURE §16.2) is then
//! `LD_LIBRARY_PATH=<pbs>/lib:<swift>/usr/lib/swift/linux:<swift>/usr/lib/swift/host/compiler` and
//! `PYTHONHOME=<pbs>`. `~/.local/opt/python-3.12` is never used (PLAN §14).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::exec::{Exec, ExecOutput};
use crate::host::HostEnv;
use crate::install;
use crate::pins::PinManifest;
use crate::sdk;
use crate::version::Version;
use crate::{Component, Provenance, SdkInfo, ToolInfo};

const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const PLATFORM_TIMEOUT: Duration = Duration::from_secs(10);

/// What the probes need.
pub(crate) struct Prober<'a> {
    pub host: &'a HostEnv,
    /// Cached probes (`--version`, `platform list`).
    pub exec: &'a dyn Exec,
    /// Uncached commands whose output changes without the program changing (`ldconfig -p`).
    pub raw: &'a dyn Exec,
    pub data_dir: &'a Path,
    pub pins: &'a PinManifest,
}

fn tool(component: Component, version: String, path: PathBuf, provenance: Provenance) -> ToolInfo {
    ToolInfo { component, version, path, provenance, env: BTreeMap::new(), ok: true, problems: Vec::new() }
}

impl ToolInfo {
    fn problem(mut self, p: impl Into<String>) -> Self {
        self.ok = false;
        self.problems.push(p.into());
        self
    }
}

async fn run(
    exec: &dyn Exec,
    path: &Path,
    args: &[&str],
    env: &BTreeMap<String, String>,
) -> Result<ExecOutput, String> {
    match exec.output(path, args, env, VERSION_TIMEOUT).await {
        Ok(out) if out.success() => Ok(out),
        Ok(out) => {
            let first = out.combined().lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_owned();
            Err(format!("`{} {}` failed: {first}", path.display(), args.join(" ")))
        }
        Err(e) => Err(e.message().to_owned()),
    }
}

/// The root of a Swift toolchain from its `swift` executable (`<root>/usr/bin/swift`, following symlinks).
pub fn swift_root(swift: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(swift).ok()?;
    let bin = real.parent()?;
    Some(bin.parent()?.parent()?.to_path_buf())
}

impl Prober<'_> {
    async fn version_of(&self, path: &Path, args: &[&str], env: &BTreeMap<String, String>) -> Result<String, String> {
        let out = run(self.exec, path, args, env).await?;
        Version::find_in(&out.combined())
            .map(|v| v.to_string())
            .ok_or_else(|| format!("no version in the output of `{} {}`", path.display(), args.join(" ")))
    }

    /// Swift: managed → system (`PATH`) → swiftly.
    pub async fn swift(&self) -> Vec<ToolInfo> {
        let mut candidates: Vec<(PathBuf, Provenance)> = Vec::new();
        for (version, dir) in install::installed(self.data_dir, "swift") {
            candidates.push((dir.join("usr/bin/swift"), Provenance::Managed { pin: format!("swift@{version}") }));
        }
        for p in self.host.which_all("swift") {
            candidates.push((p, Provenance::System));
        }
        for dir in &self.host.swiftly_dirs {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut names: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            names.sort();
            for t in names {
                candidates.push((t.join("usr/bin/swift"), Provenance::User));
            }
        }
        let mut out = Vec::new();
        for (path, provenance) in candidates {
            if !crate::host::is_executable(&path) {
                continue;
            }
            let info = match self.version_of(&path, &["--version"], &BTreeMap::new()).await {
                Ok(v) => tool(Component::Swift, v, path, provenance),
                Err(e) => tool(Component::Swift, String::new(), path, provenance).problem(e),
            };
            out.push(info);
        }
        out
    }

    /// Darwin SDKs as tools (one per SDK), with the compatibility problems a build would hit.
    pub fn darwin_sdks(&self, sdks: &[SdkInfo], swift: &[ToolInfo]) -> Vec<ToolInfo> {
        sdks.iter()
            .map(|s| {
                let provenance = if s.bundle.starts_with(self.data_dir) {
                    Provenance::Managed { pin: s.id.clone() }
                } else {
                    Provenance::XtoolBundle { bundle: s.bundle.clone() }
                };
                let mut t = tool(Component::DarwinSdk, s.ios_sdk.clone(), s.bundle.clone(), provenance);
                if s.compatible_swift.is_empty() {
                    t = t.problem(format!(
                        "no compatibility row for Xcode {} ({}) in the pin manifest",
                        s.xcode_version, s.xcode_build
                    ));
                } else if let Ok(want) = s.compatible_swift.parse::<Version>() {
                    let found = swift.iter().filter(|t| t.ok).filter_map(|t| t.version.parse::<Version>().ok());
                    if !found.clone().any(|v| v.matches_prefix(&want)) {
                        let have: Vec<String> = found.map(|v| v.to_string()).collect();
                        t = t.problem(format!(
                            "the Xcode {} SDK needs Swift {want}; found {}",
                            s.xcode_version,
                            if have.is_empty() { "none".into() } else { have.join(", ") }
                        ));
                    }
                }
                t
            })
            .collect()
    }

    /// The oldest LLD and darwin-tools that read an SDK's TBDs (the SDK's row, else the strictest row).
    pub fn linker_floor(&self, sdk: Option<&SdkInfo>) -> (Option<Version>, Option<Version>) {
        let row = sdk.and_then(|s| self.pins.xcode(&s.xcode_build).or_else(|| self.pins.xcode(&s.xcode_version)));
        let rows: Vec<_> = match row {
            Some(r) => vec![r],
            None => self.pins.xcode.iter().collect(),
        };
        let lld = rows.iter().filter_map(|r| r.min_lld.parse::<Version>().ok()).max();
        let dt = rows.iter().filter_map(|r| r.min_darwin_tools.as_deref()?.parse::<Version>().ok()).max();
        (lld, dt)
    }

    /// Linkers: each SDK bundle's darwin-tools `ld64.lld` → managed darwin-tools → system LLD (ARCHITECTURE §16.1).
    pub async fn linkers(&self, sdks: &[SdkInfo]) -> Vec<ToolInfo> {
        let mut out = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();
        for s in sdks {
            let Some(path) = sdk::bundle_linker(&s.bundle) else { continue };
            if !crate::host::is_executable(&path) {
                continue;
            }
            seen.push(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()));
            let provenance = Provenance::XtoolBundle { bundle: s.bundle.clone() };
            let (_, min_dt) = self.linker_floor(Some(s));
            out.push(self.darwin_tools_linker(path, provenance, s.darwin_tools.as_deref(), min_dt).await);
        }
        for (version, dir) in install::installed(self.data_dir, "darwin-tools") {
            let path = dir.join("bin/ld64.lld");
            if !crate::host::is_executable(&path) {
                continue;
            }
            seen.push(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()));
            let provenance = Provenance::Managed { pin: format!("darwin-tools@{version}") };
            let (_, min_dt) = self.linker_floor(sdks.first());
            out.push(self.darwin_tools_linker(path, provenance, Some(&version), min_dt).await);
        }
        let (min_lld, _) = self.linker_floor(sdks.first());
        for path in self.host.which_all("ld64.lld") {
            let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.contains(&real) {
                continue;
            }
            let info = match self.version_of(&path, &["--version"], &BTreeMap::new()).await {
                Ok(v) => {
                    let t = tool(Component::Linker, v.clone(), path, Provenance::System);
                    match (&min_lld, v.parse::<Version>()) {
                        (Some(min), Ok(found)) if found < *min => t.problem(format!(
                            "system LLD {found} is too old: the Xcode 27 SDK's arm64e.x1 TBDs need LLD >= {min} \
                             or darwin-tools"
                        )),
                        _ => t,
                    }
                }
                Err(e) => tool(Component::Linker, String::new(), path, Provenance::System).problem(e),
            };
            out.push(info);
        }
        out
    }

    async fn darwin_tools_linker(
        &self,
        path: PathBuf,
        provenance: Provenance,
        darwin_tools: Option<&str>,
        min_darwin_tools: Option<Version>,
    ) -> ToolInfo {
        let mut t = match self.version_of(&path, &["--version"], &BTreeMap::new()).await {
            Ok(v) => tool(Component::Linker, v, path, provenance),
            Err(e) => return tool(Component::Linker, String::new(), path, provenance).problem(e),
        };
        match (darwin_tools.and_then(|d| d.parse::<Version>().ok()), min_darwin_tools) {
            (Some(found), Some(min)) if found < min => {
                t = t.problem(format!("darwin-tools {found} is older than {min}, which reads arm64e.x1 TBDs"));
            }
            (None, _) => t = t.problem("the bundle does not say which darwin-tools it carries"),
            _ => {}
        }
        t
    }

    /// The `libpython` sonames `liblldb` needs (`libpython3.12.so.1.0`), from its ELF `DT_NEEDED`.
    pub fn liblldb_python_needs(&self, swift_root: &Path) -> Vec<String> {
        let lib = swift_root.join("usr/lib");
        let Ok(entries) = std::fs::read_dir(&lib) else { return Vec::new() };
        let mut libs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("liblldb.so")))
            .collect();
        libs.sort();
        for l in libs {
            let Ok(bytes) = std::fs::read(&l) else { continue };
            let Ok(elf) = goblin::elf::Elf::parse(&bytes) else { continue };
            let needs: Vec<String> =
                elf.libraries.iter().filter(|n| n.starts_with("libpython")).map(|n| (*n).to_owned()).collect();
            return needs;
        }
        Vec::new()
    }

    /// Candidates for a libpython soname: `ldconfig` → uv-managed PBS → Russet's managed `python-shared`.
    pub async fn libpython(&self, soname: &str) -> Vec<ToolInfo> {
        let mut out = Vec::new();
        let minor = soname.strip_prefix("libpython").and_then(|s| s.split(".so").next()).unwrap_or("").to_owned();
        // 1. The system's shared-library cache.
        if let Some(ldconfig) = &self.host.ldconfig
            && let Ok(o) = self.raw.output(ldconfig, &["-p"], &BTreeMap::new(), VERSION_TIMEOUT).await
        {
            for line in o.stdout.lines() {
                let line = line.trim();
                if !line.starts_with(soname) {
                    continue;
                }
                if let Some((_, path)) = line.split_once("=>") {
                    let path = PathBuf::from(path.trim());
                    if path.is_file() && !self.host.is_retired(&path) {
                        out.push(tool(Component::LibPython, minor.clone(), path, Provenance::System));
                    }
                }
            }
        }
        // 2. uv's python-build-standalone installs (`cpython-3.12.15-linux-x86_64-gnu`), newest first.
        let mut uv: Vec<(Version, PathBuf)> = Vec::new();
        for dir in &self.host.uv_python_dirs {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for e in entries.filter_map(Result::ok) {
                let path = e.path();
                if path.is_symlink() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
                let Some(rest) = name.strip_prefix("cpython-") else { continue };
                let Some(ver) = rest.split('-').next().and_then(|v| v.parse::<Version>().ok()) else { continue };
                let lib = path.join("lib").join(soname);
                if ver.to_string().starts_with(&format!("{minor}.")) && lib.is_file() && !self.host.is_retired(&lib) {
                    uv.push((ver, lib));
                }
            }
        }
        uv.sort_by(|a, b| b.0.cmp(&a.0));
        for (ver, lib) in uv {
            let mut t = tool(Component::LibPython, ver.to_string(), lib.clone(), Provenance::Uv);
            if let Some(home) = lib.parent().and_then(Path::parent) {
                t.env.insert("PYTHONHOME".into(), home.display().to_string());
            }
            out.push(t);
        }
        // 3. Russet's managed python-shared.
        for (version, dir) in install::installed(self.data_dir, "python-shared") {
            let lib = dir.join("lib").join(soname);
            if lib.is_file() && version.starts_with(&format!("{minor}.")) {
                let mut t = tool(
                    Component::LibPython,
                    version.clone(),
                    lib,
                    Provenance::Managed { pin: format!("python-shared@{version}") },
                );
                t.env.insert("PYTHONHOME".into(), dir.display().to_string());
                out.push(t);
            }
        }
        out
    }

    /// The env contract of a Swift-toolchain LLDB (ARCHITECTURE §16.2).
    pub fn lldb_env(swift_root: &Path, python: Option<&ToolInfo>) -> BTreeMap<String, String> {
        let mut dirs: Vec<String> = Vec::new();
        let mut env = BTreeMap::new();
        if let Some(py) = python {
            if let Some(home) = py.env.get("PYTHONHOME") {
                dirs.push(format!("{home}/lib"));
                env.insert("PYTHONHOME".to_owned(), home.clone());
            } else if let Some(dir) = py.path.parent() {
                dirs.push(dir.display().to_string());
            }
        }
        dirs.push(swift_root.join("usr/lib/swift/linux").display().to_string());
        dirs.push(swift_root.join("usr/lib/swift/host/compiler").display().to_string());
        env.insert("LD_LIBRARY_PATH".to_owned(), dirs.join(":"));
        env
    }

    /// LLDB or lldb-dap of the `swift` flavor, for every Swift toolchain that is `ok`.
    pub async fn swift_lldb(&self, swift: &[ToolInfo], dap: bool) -> Vec<ToolInfo> {
        let component = if dap { Component::LldbDap } else { Component::Lldb };
        let name = if dap { "lldb-dap" } else { "lldb" };
        let mut out = Vec::new();
        let mut roots: Vec<PathBuf> = Vec::new();
        for s in swift.iter().filter(|s| s.ok) {
            let Some(root) = swift_root(&s.path) else { continue };
            if roots.contains(&root) {
                continue;
            }
            roots.push(root.clone());
            let path = root.join("usr/bin").join(name);
            if !crate::host::is_executable(&path) {
                continue;
            }
            let provenance = s.provenance.clone();
            let needs = if cfg!(target_os = "linux") { self.liblldb_python_needs(&root) } else { Vec::new() };
            let mut python = None;
            let mut missing = Vec::new();
            for soname in &needs {
                match self.libpython(soname).await.into_iter().find(|p| p.ok) {
                    Some(p) => python = Some(p),
                    None => missing.push(soname.clone()),
                }
            }
            let env = Self::lldb_env(&root, python.as_ref());
            let mut t = tool(component, String::new(), path.clone(), provenance);
            t.env = env.clone();
            if !missing.is_empty() {
                out.push(t.problem(format!(
                    "liblldb needs {}, which is not installed; run `russet toolchain install python-shared`",
                    missing.join(", ")
                )));
                continue;
            }
            let version_args: &[&str] = &["--version"];
            match self.version_of(&path, version_args, &env).await {
                Ok(v) => t.version = v,
                Err(e) => {
                    out.push(t.problem(e));
                    continue;
                }
            }
            if !dap {
                match self.exec.output(&path, &["-b", "-o", "platform list"], &env, PLATFORM_TIMEOUT).await {
                    Ok(o) if o.combined().contains("remote-ios") => {}
                    Ok(_) => t = t.problem("this LLDB has no remote-ios platform"),
                    Err(e) => t = t.problem(e.message().to_owned()),
                }
            }
            out.push(t);
        }
        out
    }

    /// LLDB or lldb-dap of the `llvm` flavor: a distro LLDB ≥ 23.1 on `PATH` (RD §8.1).
    pub async fn llvm_lldb(&self, swift_roots: &[PathBuf], dap: bool) -> Vec<ToolInfo> {
        let component = if dap { Component::LldbDap } else { Component::Lldb };
        let name = if dap { "lldb-dap" } else { "lldb" };
        let mut out = Vec::new();
        for path in self.host.which_all(name) {
            let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if swift_roots.iter().any(|r| real.starts_with(r)) {
                continue;
            }
            let t = match self.version_of(&path, &["--version"], &BTreeMap::new()).await {
                Ok(v) => {
                    let t = tool(component, v.clone(), path, Provenance::System);
                    match v.parse::<Version>() {
                        Ok(found) if found < Version::new(vec![23, 1]) => {
                            t.problem(format!("LLDB {found} is older than 23.1 (RD §8.1)"))
                        }
                        _ => t,
                    }
                }
                Err(e) => tool(component, String::new(), path, Provenance::System).problem(e),
            };
            out.push(t);
        }
        out
    }

    /// A tool on `PATH` that prints its version with `--version` (xtool, flutter).
    pub async fn simple(&self, component: Component, name: &str) -> Vec<ToolInfo> {
        let mut out = Vec::new();
        if let Some(path) = self.host.which(name) {
            out.push(match self.version_of(&path, &["--version"], &BTreeMap::new()).await {
                Ok(v) => tool(component, v, path, Provenance::System),
                Err(e) => tool(component, String::new(), path, Provenance::System).problem(e),
            });
        }
        out
    }
}
