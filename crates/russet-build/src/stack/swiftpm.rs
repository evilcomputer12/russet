// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The SwiftPM adapter (build §4.1–§4.3, B03): Russet's own builder package and `swift build --build-system
//! swiftbuild`, re-implementing xtool's technique (MIT, xtool-org/xtool `PackLib`) in Rust.
//!
//! - **Planning** runs `swift package resolve`, `show-dependencies` and `describe` and picks the app's library
//!   product, its extensions' products, the resource bundles (`<Package>_<Target>.bundle`), binary frameworks and
//!   `.dynamic` products. A graph with `.dynamic` products builds with the `native` backend, because Swift Build on
//!   Linux cannot link them into the builder executable [V build V19] (B04 finishes this, W16).
//! - **The builder package** in `<project>/.russet/work/builder/` has one executable target per product
//!   (`<Product>-App`, `<Product>-Extension`) that depends on the app package's library product; extensions link
//!   with `-e _NSExtensionMain`.
//! - **The invocation** gives the environment explicitly: never `SDKROOT` (it breaks SwiftPM's host manifest link
//!   [V build V4]), `XCODE_EXTRA_PLATFORM_FOLDERS` = the platform view, the SDK's linker directory first on `PATH`
//!   (Swift Build finds `dsymutil` there), the toolchain's `usr/lib/swift/linux` as `LD_LIBRARY_PATH` (its lld
//!   needs the libraries bundled there on newer distros), and only an allowlist of the caller's variables, so
//!   Swift Build's failure dumps cannot leak secrets [V build V7]. The SDK fix lives in the toolset, so dependency
//!   dylibs get `sdk 27.0` too.
//! - **Output** `[n / m]` lines become `progress` events and `file:line:col: error|warning|note:` lines
//!   `diagnostic` events; lld's benign `does not support linking for platform iOS` is dropped and `env -i …` dumps
//!   are redacted.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::sync::Mutex;

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::proc::{CommandOutput, CommandSpec};
use russet_core::progress::{EventSink, JobEventKind, LogStream, ProgressSink};
use russet_sdk::{SdkFacts, Version};
use serde::Deserialize;

use crate::events::{BuildEvent, Diagnostic, Emitter, Severity};
use crate::manifest::Backend;

/// The deployment target when neither `russet.toml` nor `Package.swift` names one (build §4.2).
pub const DEFAULT_DEPLOYMENT_TARGET: Version = Version::new(15, 0, 0);

const GRAPH_HINT: &str = "SwiftPM's package description is inconsistent; rebuild with `russet build --clean`, and report it with `russet bugreport` if it persists.";

/// What `swift package describe --type json` says about one package (the fields Russet reads).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PackageDump {
    pub name: String,
    #[serde(default)]
    pub products: Vec<DumpProduct>,
    #[serde(default)]
    pub targets: Vec<DumpTarget>,
    #[serde(default)]
    pub platforms: Vec<DumpPlatform>,
}

/// A product of a [`PackageDump`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DumpProduct {
    pub name: String,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(rename = "type")]
    pub kind: serde_json::Value,
}

/// How a product links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductType {
    Executable,
    Automatic,
    Static,
    Dynamic,
    Other,
}

impl DumpProduct {
    /// `{"library": ["automatic"]}`, `{"executable": null}`, …
    pub fn product_type(&self) -> ProductType {
        let Some(obj) = self.kind.as_object() else { return ProductType::Other };
        if obj.contains_key("executable") {
            return ProductType::Executable;
        }
        match obj
            .get("library")
            .and_then(|l| l.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        {
            Some(kinds) if kinds == ["automatic"] => ProductType::Automatic,
            Some(kinds) if kinds == ["static"] => ProductType::Static,
            Some(kinds) if kinds == ["dynamic"] => ProductType::Dynamic,
            _ => ProductType::Other,
        }
    }
}

/// A target of a [`PackageDump`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DumpTarget {
    pub name: String,
    #[serde(default)]
    pub module_type: String,
    #[serde(default)]
    pub product_dependencies: Vec<String>,
    #[serde(default)]
    pub target_dependencies: Vec<String>,
    #[serde(default)]
    pub resources: Vec<serde_json::Value>,
}

/// A platform requirement of a [`PackageDump`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DumpPlatform {
    pub name: String,
    pub version: String,
}

/// One node of `swift package show-dependencies --format json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DependencyNode {
    pub identity: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub dependencies: Vec<DependencyNode>,
}

/// A product the builder package turns into an executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductPlan {
    /// The library product (and the bundle's executable name).
    pub product: String,
    /// `<product>-App` or `<product>-Extension`.
    pub target_name: String,
    pub is_extension: bool,
    /// `<Package>_<Target>.bundle` resource bundles to embed.
    pub resource_bundles: Vec<String>,
    /// Binary targets (`<name>.framework`, embedded unless static).
    pub binary_frameworks: Vec<String>,
    /// `.dynamic` products (`lib<name>.dylib`).
    pub dynamic_libraries: Vec<String>,
}

/// The plan of one SwiftPM build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwiftpmPlan {
    pub package_dir: Utf8PathBuf,
    pub package_name: String,
    pub app: ProductPlan,
    pub extensions: Vec<ProductPlan>,
    /// The package's `.iOS` platform, when it declares one.
    pub package_deployment_target: Option<Version>,
}

impl SwiftpmPlan {
    /// The app first, then the extensions.
    pub fn products(&self) -> impl Iterator<Item = &ProductPlan> {
        std::iter::once(&self.app).chain(&self.extensions)
    }

    /// Whether any product embeds a `.dynamic` library (the `native` fallback, B04).
    pub fn has_dynamic_products(&self) -> bool {
        self.products().any(|p| !p.dynamic_libraries.is_empty())
    }
}

/// The package graph.
#[derive(Debug, Clone, Default)]
pub struct PackageGraph {
    pub root: PackageDump,
    /// Packages by identity.
    pub packages: BTreeMap<String, PackageDump>,
    /// Package identity by product name.
    pub product_owner: BTreeMap<String, String>,
    pub root_identity: String,
}

impl PackageGraph {
    /// Builds the graph from the dependency tree and one dump per package.
    pub fn new(deps: &DependencyNode, mut dumps: BTreeMap<String, PackageDump>) -> Result<Self> {
        let root = dumps.get(&deps.identity).cloned().ok_or_else(|| {
            Error::new(codes::core::INTERNAL, format!("no description of the root package {}", deps.identity))
                .with_hint(GRAPH_HINT)
        })?;
        let mut product_owner = BTreeMap::new();
        for (id, dump) in &dumps {
            for p in &dump.products {
                product_owner.entry(p.name.clone()).or_insert_with(|| id.clone());
            }
        }
        dumps.entry(deps.identity.clone()).or_insert_with(|| root.clone());
        Ok(Self { root, packages: dumps, product_owner, root_identity: deps.identity.clone() })
    }

    fn product(&self, name: &str) -> Option<(&PackageDump, &DumpProduct)> {
        let pkg = self.packages.get(self.product_owner.get(name)?)?;
        Some((pkg, pkg.products.iter().find(|p| p.name == name)?))
    }

    /// Plans the app product (`wanted`, or the only library product) and the extension products.
    pub fn plan(&self, package_dir: &Utf8Path, wanted: Option<&str>, extensions: &[String]) -> Result<SwiftpmPlan> {
        let libraries: Vec<&DumpProduct> =
            self.root.products.iter().filter(|p| p.product_type() == ProductType::Automatic).collect();
        let app = match (wanted, libraries.as_slice()) {
            (Some(w), _) => libraries.iter().find(|p| p.name == w).copied().ok_or_else(|| {
                Error::new(codes::config::INVALID, format!("the package has no library product `{w}`"))
                    .with_hint(format!("`[build.swiftpm] product` must name one of: {}.", names(&libraries)))
            })?,
            (None, [one]) => *one,
            (None, []) => {
                return Err(Error::new(codes::config::INVALID, "the package has no library product for the app")
                    .with_hint(
                        "Declare `.library(name: \"<App>\", targets: [\"<App>\"])` in Package.swift (build §4.2).",
                    ));
            }
            (None, many) => {
                return Err(Error::new(
                    codes::config::INVALID,
                    format!("the package has several library products: {}", names(many)),
                )
                .with_hint("Name the app's product in `[build.swiftpm] product`."));
            }
        };
        let app = self.product_plan(app, false)?;
        let mut ext = Vec::new();
        for name in extensions {
            let p = self.root.products.iter().find(|p| &p.name == name).ok_or_else(|| {
                Error::new(
                    codes::config::INVALID,
                    format!("`[[app.extensions]] product = \"{name}\"` is not a product of the package"),
                )
                .with_hint("Each extension is a library product of the app's package.")
            })?;
            ext.push(self.product_plan(p, true)?);
        }
        let package_deployment_target =
            self.root.platforms.iter().find(|p| p.name == "ios").and_then(|p| Version::parse(&p.version));
        Ok(SwiftpmPlan {
            package_dir: package_dir.to_owned(),
            package_name: self.root.name.clone(),
            app,
            extensions: ext,
            package_deployment_target,
        })
    }

    fn product_plan(&self, product: &DumpProduct, is_extension: bool) -> Result<ProductPlan> {
        let mut resource_bundles = BTreeSet::new();
        let mut binary_frameworks = BTreeSet::new();
        let mut dynamic_libraries = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut stack: Vec<(String, String)> =
            product.targets.iter().map(|t| (self.root_identity.clone(), t.clone())).collect();
        while let Some((pkg_id, target_name)) = stack.pop() {
            if !visited.insert((pkg_id.clone(), target_name.clone())) {
                continue;
            }
            let pkg = self.packages.get(&pkg_id).ok_or_else(|| {
                Error::new(codes::core::INTERNAL, format!("package {pkg_id} is missing from the graph"))
                    .with_hint(GRAPH_HINT)
            })?;
            let target = pkg.targets.iter().find(|t| t.name == target_name).ok_or_else(|| {
                Error::new(codes::core::INTERNAL, format!("target {target_name} is missing from package {}", pkg.name))
                    .with_hint(GRAPH_HINT)
            })?;
            if target.module_type == "BinaryTarget" {
                binary_frameworks.insert(target_name.clone());
            }
            if !target.resources.is_empty() {
                resource_bundles.insert(format!("{}_{}.bundle", pkg.name, target_name));
            }
            for dep in &target.target_dependencies {
                stack.push((pkg_id.clone(), dep.clone()));
            }
            for dep in &target.product_dependencies {
                let Some((owner, p)) = self.product(dep) else { continue };
                if p.product_type() == ProductType::Dynamic {
                    dynamic_libraries.insert(dep.clone());
                }
                let owner_id = self.product_owner.get(dep).cloned().unwrap_or_default();
                let _ = owner;
                for t in &p.targets {
                    stack.push((owner_id.clone(), t.clone()));
                }
            }
        }
        let suffix = if is_extension { "Extension" } else { "App" };
        Ok(ProductPlan {
            product: product.name.clone(),
            target_name: format!("{}-{suffix}", product.name),
            is_extension,
            resource_bundles: resource_bundles.into_iter().collect(),
            binary_frameworks: binary_frameworks.into_iter().collect(),
            dynamic_libraries: dynamic_libraries.into_iter().collect(),
        })
    }
}

fn names(products: &[&DumpProduct]) -> String {
    products.iter().map(|p| format!("`{}`", p.name)).collect::<Vec<_>>().join(", ")
}

/// Parses `describe` output; SwiftPM may print other text before the JSON (xtool's observation).
pub fn parse_describe(stdout: &[u8]) -> Result<PackageDump> {
    let start = stdout.iter().position(|b| *b == b'{').unwrap_or(stdout.len());
    serde_json::from_slice(&stdout[start..]).map_err(|e| {
        Error::new(codes::build::COMPILE_FAILED, format!("cannot read `swift package describe` output: {e}"))
            .with_hint("Check that Package.swift evaluates (`swift package describe`).")
    })
}

/// The generated `Package.swift` of the builder package.
pub fn builder_manifest(plan: &SwiftpmPlan, deployment_target: Version, app_package_path: &str) -> String {
    let mut targets = Vec::new();
    for p in plan.products() {
        let linker = if p.is_extension {
            ".linkedFramework(\"Foundation\"), .unsafeFlags([\"-Xlinker\", \"-e\", \"-Xlinker\", \"_NSExtensionMain\", \
             \"-Xlinker\", \"-rpath\", \"-Xlinker\", \"@executable_path/../../Frameworks\", \"-Xlinker\", \"-rpath\", \
             \"-Xlinker\", \"@executable_path/Frameworks\"])"
        } else {
            ".unsafeFlags([\"-Xlinker\", \"-rpath\", \"-Xlinker\", \"@executable_path/Frameworks\"])"
        };
        targets.push(format!(
            "    .executableTarget(\n      name: {name}, dependencies: [.product(name: {product}, package: \"App\")], path: {path},\n      \
             linkerSettings: [{linker}]),",
            name = swift_string(&p.target_name),
            product = swift_string(&p.product),
            path = swift_string(&format!("Sources/{}", p.target_name)),
        ));
    }
    format!(
        "// swift-tools-version: 6.0\n// Generated by Russet. Do not edit.\nimport PackageDescription\nlet package = Package(\n  \
         name: \"RussetBuilder\",\n  platforms: [.iOS({dt})],\n  dependencies: [.package(name: \"App\", path: {path})],\n  \
         targets: [\n{targets}\n  ])\n",
        dt = swift_string(&deployment_target.to_string()),
        path = swift_string(app_package_path),
        targets = targets.join("\n"),
    )
}

fn swift_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Writes `contents` to `path` unless it already holds them (SwiftPM re-evaluates a touched manifest).
pub fn write_if_changed(path: &Utf8Path, contents: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == contents) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    std::fs::write(path, contents).map_err(|e| io(path, e))
}

pub(crate) fn io(path: &Utf8Path, e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::StorageFull {
        return Error::new(codes::core::DISK_FULL, format!("{path}: {e}"))
            .with_hint("Free some disk space (`russet clean --all` removes this project's build files), then retry.")
            .caused_by(e);
    }
    Error::new(codes::core::IO, format!("{path}: {e}"))
        .with_hint("Check that the path exists and that your user can read and write it.")
        .caused_by(e)
}

/// `name` as one path component: characters other than ASCII letters, digits, `.`, `-` and `_` become `_`.
pub fn path_component(name: &str) -> String {
    let s: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    if s.is_empty() || s.chars().all(|c| c == '.') { format!("_{s}") } else { s }
}

/// Writes the builder package (manifest and one empty `stub.c` per target) and returns its directory.
pub fn write_builder(work: &Utf8Path, plan: &SwiftpmPlan, deployment_target: Version) -> Result<Utf8PathBuf> {
    let dir = work.join("builder");
    let rel = relative_path(&dir, &plan.package_dir);
    write_if_changed(&dir.join("Package.swift"), builder_manifest(plan, deployment_target, &rel).as_bytes())?;
    for p in plan.products() {
        write_if_changed(&dir.join("Sources").join(&p.target_name).join("stub.c"), b"")?;
    }
    Ok(dir)
}

/// `to` relative to `from` (both absolute), with `/` separators.
pub fn relative_path(from: &Utf8Path, to: &Utf8Path) -> String {
    let from: Vec<&str> = from.components().map(|c| c.as_str()).collect();
    let to: Vec<&str> = to.components().map(|c| c.as_str()).collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    if parts.is_empty() { ".".to_owned() } else { parts.join("/") }
}

/// Swift Build's or SwiftPM's products directory for a configuration.
pub fn products_dir(scratch: &Utf8Path, backend: Backend, release: bool) -> Utf8PathBuf {
    match backend {
        Backend::Native => scratch.join("arm64-apple-ios").join(if release { "release" } else { "debug" }),
        _ => scratch.join("out/Products").join(if release { "Release-iphoneos" } else { "Debug-iphoneos" }),
    }
}

/// The caller's environment variables a SwiftPM build may see; everything else (tokens, keys) stays out.
const ENV_ALLOWLIST: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "TEMP",
    "TMP",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
    "SOURCE_DATE_EPOCH",
    "SWIFTPM_CUSTOM_BIN_DIR",
    // Windows needs these to start processes at all.
    "SystemRoot",
    "windir",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ProgramFiles",
    "PATHEXT",
    "ComSpec",
];

/// The toolchain's runtime library directory (`<root>/usr/lib/swift/linux`), when it exists. Swift Build runs the
/// toolchain's own lld while it plans a link, and a toolchain built for an older distro finds the libraries it
/// bundles there (`libxml2.so.2` on Ubuntu 26.04) only through `LD_LIBRARY_PATH`, as LLDB does (ARCHITECTURE
/// §16.2). Without them lld cannot start, and planning fails with "Could not parse output of `ld.lld
/// -version_details` as JSON".
pub fn toolchain_library_dir(swift: &Utf8Path) -> Option<Utf8PathBuf> {
    let dir = swift.parent()?.parent()?.join("lib/swift/linux");
    dir.is_dir().then_some(dir)
}

/// The explicit environment of a SwiftPM process: the allowlist, `PATH` with `prepend` first, `extra`; never
/// `SDKROOT` (ARCHITECTURE §16.4; build §4.2).
pub fn tool_env(prepend: &[&Utf8Path], extra: &[(&str, &str)]) -> BTreeMap<OsString, OsString> {
    tool_env_from(|key| std::env::var_os(key), prepend, extra)
}

/// [`tool_env`] with the caller's variables read from `caller` instead of this process's environment.
pub fn tool_env_from(
    caller: impl Fn(&str) -> Option<OsString>,
    prepend: &[&Utf8Path],
    extra: &[(&str, &str)],
) -> BTreeMap<OsString, OsString> {
    let mut env = BTreeMap::new();
    for key in ENV_ALLOWLIST {
        if let Some(v) = caller(key) {
            env.insert(OsString::from(key), v);
        }
    }
    let mut paths: Vec<std::path::PathBuf> = prepend.iter().map(|p| p.as_std_path().to_owned()).collect();
    if let Some(existing) = caller("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        env.insert(OsString::from("PATH"), joined);
    }
    for (k, v) in extra {
        env.insert(OsString::from(k), OsString::from(v));
    }
    env.remove(&OsString::from("SDKROOT"));
    env
}

/// The `swift build` command for a plan.
#[derive(Debug, Clone)]
pub struct Invocation<'a> {
    pub facts: &'a SdkFacts,
    pub backend: Backend,
    pub release: bool,
    pub builder_dir: &'a Utf8Path,
    pub scratch: &'a Utf8Path,
    pub cwd: &'a Utf8Path,
    /// The toolset to use (the SDK's, or the diagnosis-only toolset without the SDK fix).
    pub toolset: &'a Utf8Path,
    pub jobs: Option<u32>,
    pub swiftc_flags: &'a [String],
    pub cc_flags: &'a [String],
    pub ld_flags: &'a [String],
}

impl Invocation<'_> {
    /// The command line and environment.
    pub fn spec(&self) -> CommandSpec {
        let mut args: Vec<OsString> = Vec::new();
        let mut push = |s: &str| args.push(OsString::from(s));
        push("build");
        push("--configuration");
        push(if self.release { "release" } else { "debug" });
        push("--build-system");
        match self.backend {
            Backend::Native => {
                push("native");
                push("--swift-sdk");
                push("arm64-apple-ios");
                if let Some(parent) = self.facts.bundle.parent() {
                    push("--swift-sdks-path");
                    push(parent.as_str());
                }
            }
            _ => {
                push("swiftbuild");
                push("--triple");
                push("arm64-apple-ios");
            }
        }
        push("--toolset");
        push(self.toolset.as_str());
        push("--package-path");
        push(self.builder_dir.as_str());
        push("--scratch-path");
        push(self.scratch.as_str());
        push("--disable-automatic-resolution");
        if let Some(j) = self.jobs {
            push("-j");
            push(&j.to_string());
        }
        for (flag, values) in [("-Xswiftc", self.swiftc_flags), ("-Xcc", self.cc_flags), ("-Xlinker", self.ld_flags)] {
            for v in values {
                push(flag);
                push(v);
            }
        }
        let tools = &self.facts.tools;
        let swift_bin = tools.swift.parent().unwrap_or(Utf8Path::new(""));
        let library_dir = toolchain_library_dir(&tools.swift);
        let mut extra = vec![("XCODE_EXTRA_PLATFORM_FOLDERS", self.facts.platforms_view.as_str())];
        if let Some(dir) = &library_dir {
            extra.push(("LD_LIBRARY_PATH", dir.as_str()));
        }
        let env = tool_env(&[&tools.linker_dir, swift_bin], &extra);
        CommandSpec {
            program: tools.swift.as_std_path().to_owned(),
            args,
            env,
            env_clear: true,
            cwd: Some(self.cwd.as_std_path().to_owned()),
            nice: Some(10),
            timeout: None,
        }
    }
}

/// A `swift package <args>` command for the package at `package`. Give every package a `scratch` under
/// `.russet/work`: without one SwiftPM creates `.build/` in the package's own directory.
pub fn package_command(swift: &Utf8Path, package: &Utf8Path, scratch: Option<&Utf8Path>, args: &[&str]) -> CommandSpec {
    let mut a: Vec<OsString> = vec!["package".into(), "--package-path".into(), package.as_str().into()];
    if let Some(s) = scratch {
        a.push("--scratch-path".into());
        a.push(s.as_str().into());
    }
    a.extend(args.iter().map(OsString::from));
    let swift_bin = swift.parent().unwrap_or(Utf8Path::new(""));
    CommandSpec {
        program: swift.as_std_path().to_owned(),
        args: a,
        env: tool_env(&[swift_bin], &[]),
        env_clear: true,
        cwd: Some(package.as_std_path().to_owned()),
        nice: Some(10),
        timeout: None,
    }
}

/// Parses one line of build output into an event; `None` drops the line.
pub fn parse_line(stream: LogStream, line: &str) -> Option<BuildEvent> {
    let trimmed = line.trim_end();
    if trimmed.contains("does not support linking for platform iOS") {
        return None;
    }
    if let Some(event) = parse_progress(trimmed) {
        return Some(event);
    }
    if let Some(mut d) = parse_diagnostic(trimmed) {
        d.message = redact_env_dump(&d.message);
        return Some(BuildEvent::Diagnostic { diagnostic: d });
    }
    Some(BuildEvent::Log { stream, line: redact_env_dump(trimmed) })
}

fn parse_progress(line: &str) -> Option<BuildEvent> {
    let rest = line.strip_prefix('[')?;
    let (inside, tail) = rest.split_once(']')?;
    let (done, total) = inside.split_once('/')?;
    let done: u64 = done.trim().parse().ok()?;
    let total: u64 = total.trim().parse().ok()?;
    let message = Some(tail.trim()).filter(|m| !m.is_empty()).map(str::to_owned);
    Some(BuildEvent::Progress { done: Some(done), total: Some(total), message })
}

/// `file:line:col: error: message`, `file:line: warning: …`, `tool: error: …`, or a bare `error: …`.
pub fn parse_diagnostic(line: &str) -> Option<Diagnostic> {
    for (marker, severity) in
        [(": error: ", Severity::Error), (": warning: ", Severity::Warning), (": note: ", Severity::Note)]
    {
        let Some(pos) = line.find(marker) else { continue };
        let location = &line[..pos];
        let message = line[pos + marker.len()..].to_owned();
        let (file, line_no, column) = split_location(location);
        let tool = TOOLS.iter().find(|t| file.as_deref().is_some_and(|f| f == **t || f.ends_with(&format!("/{t}"))));
        let (file, tool) = match tool {
            Some(t) => (None, Some((*t).to_owned())),
            None => (file, None),
        };
        return Some(Diagnostic { severity, message, file, line: line_no, column, tool, code: None });
    }
    for (prefix, severity) in [("error: ", Severity::Error), ("warning: ", Severity::Warning)] {
        if let Some(message) = line.strip_prefix(prefix) {
            return Some(Diagnostic {
                severity,
                message: message.to_owned(),
                file: None,
                line: None,
                column: None,
                tool: None,
                code: None,
            });
        }
    }
    None
}

/// Tools that prefix their own messages (`ld64.lld: error: …`).
const TOOLS: &[&str] = &["ld64.lld", "ld", "clang", "swift-frontend", "swift-driver", "swiftc", "actool", "llvm-lib"];

/// `file:line:col`, `file:line` or `file`.
fn split_location(location: &str) -> (Option<String>, Option<u32>, Option<u32>) {
    let mut file = location;
    let mut numbers = Vec::new();
    for _ in 0..2 {
        match file.rsplit_once(':') {
            Some((head, tail)) if tail.parse::<u32>().is_ok() => {
                numbers.push(tail.parse::<u32>().unwrap_or_default());
                file = head;
            }
            _ => break,
        }
    }
    numbers.reverse();
    let file = Some(file.trim().to_owned()).filter(|f| !f.is_empty());
    (file, numbers.first().copied(), numbers.get(1).copied())
}

/// Replaces the values of `KEY=VALUE` words on a line that dumps an environment (`env -i …`).
pub fn redact_env_dump(line: &str) -> String {
    if !line.contains("env -i ") && !line.contains("/usr/bin/env -i") {
        return line.to_owned();
    }
    line.split(' ')
        .map(|word| match word.split_once('=') {
            Some((k, _))
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') =>
            {
                format!("{k}=<redacted>")
            }
            _ => word.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// An [`EventSink`] for a running tool: output lines become build events; everything else is forwarded.
pub struct LineSink<'e, 'a> {
    emitter: &'e Emitter<'a>,
    tail: Mutex<Vec<String>>,
}

impl<'e, 'a> LineSink<'e, 'a> {
    pub fn new(emitter: &'e Emitter<'a>) -> Self {
        Self { emitter, tail: Mutex::new(Vec::new()) }
    }

    /// The last output lines (for error messages).
    pub fn tail(&self) -> Vec<String> {
        self.tail.lock().map(|t| t.clone()).unwrap_or_default()
    }
}

impl ProgressSink for LineSink<'_, '_> {
    fn begin(&self, _title: &str, _cancellable: bool) {}
    fn report(&self, _msg: Option<&str>, _pct: Option<u8>) {}
    fn end(&self, _msg: Option<&str>) {}
    fn log(&self, stream: LogStream, line: &str) {
        if let Some(event) = parse_line(stream, line) {
            if let BuildEvent::Log { line, .. } = &event
                && let Ok(mut t) = self.tail.lock()
            {
                t.push(line.clone());
                if t.len() > 40 {
                    t.remove(0);
                }
            }
            self.emitter.emit(event);
        }
    }
}

impl EventSink for LineSink<'_, '_> {
    fn emit(&self, _e: JobEventKind) {}
}

/// Whether a command succeeded.
pub fn succeeded(out: &CommandOutput) -> bool {
    out.code == Some(0)
}

/// `build.compileFailed` or `build.linkFailed` from a failed `swift build`, with the first errors attached.
pub fn build_failure(diagnostics: &[Diagnostic], tail: &[String], log: Option<&Utf8Path>) -> Error {
    let errors: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
    let link = errors.iter().any(|d| {
        d.tool.as_deref() == Some("ld64.lld")
            || d.message.contains("undefined symbol")
            || d.message.contains("linker command failed")
            || d.message.contains("Linking")
    }) || tail.iter().any(|l| l.contains("ld64.lld: error"));
    let code = if link { codes::build::LINK_FAILED } else { codes::build::COMPILE_FAILED };
    let first = errors.first().map(|d| crate::events::render_diagnostic(d)).or_else(|| tail.last().cloned());
    let what = if link { "linking failed" } else { "compilation failed" };
    let message = match first {
        Some(f) => format!("{what}: {f}"),
        None => what.to_owned(),
    };
    let first20: Vec<&Diagnostic> = errors.iter().take(20).copied().collect();
    let hint = match log {
        Some(l) => format!("Fix the errors listed in the build's diagnostics; the full log is {l}."),
        None => "Fix the errors listed in the build's diagnostics.".to_owned(),
    };
    Error::new(code, message)
        .with_hint(hint)
        .with_details(serde_json::json!({ "errors": first20, "errorCount": errors.len() }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const DESCRIBE: &str = r#"warning: some noise
{
  "name" : "RussetProbe",
  "platforms" : [ { "name" : "ios", "version" : "17.0" } ],
  "products" : [
    { "name" : "RussetProbe", "targets" : [ "RussetProbe" ], "type" : { "library" : [ "automatic" ] } },
    { "name" : "Widget", "targets" : [ "Widget" ], "type" : { "library" : [ "automatic" ] } },
    { "name" : "Dyn", "targets" : [ "Dyn" ], "type" : { "library" : [ "dynamic" ] } },
    { "name" : "tool", "targets" : [ "tool" ], "type" : { "executable" : null } }
  ],
  "targets" : [
    { "name" : "RussetProbe", "module_type" : "SwiftTarget", "target_dependencies" : [ "Core" ], "product_dependencies" : [ "Dyn" ] },
    { "name" : "Core", "module_type" : "SwiftTarget", "resources" : [ { "path" : "a.json" } ], "target_dependencies" : [ "Lib" ] },
    { "name" : "Lib", "module_type" : "BinaryTarget" },
    { "name" : "Widget", "module_type" : "SwiftTarget" },
    { "name" : "Dyn", "module_type" : "SwiftTarget" },
    { "name" : "tool", "module_type" : "SwiftTarget" }
  ]
}"#;

    fn graph() -> PackageGraph {
        let dump = parse_describe(DESCRIBE.as_bytes()).unwrap();
        let deps = DependencyNode {
            identity: "russetprobe".into(),
            name: "RussetProbe".into(),
            path: "/p".into(),
            dependencies: Vec::new(),
        };
        PackageGraph::new(&deps, BTreeMap::from([("russetprobe".to_owned(), dump)])).unwrap()
    }

    #[test]
    fn the_plan_follows_targets_and_products() {
        let plan = graph().plan(Utf8Path::new("/p"), Some("RussetProbe"), &["Widget".into()]).unwrap();
        assert_eq!(plan.app.target_name, "RussetProbe-App");
        assert_eq!(plan.app.resource_bundles, vec!["RussetProbe_Core.bundle".to_owned()]);
        assert_eq!(plan.app.binary_frameworks, vec!["Lib".to_owned()]);
        assert_eq!(plan.app.dynamic_libraries, vec!["Dyn".to_owned()]);
        assert!(plan.has_dynamic_products());
        assert_eq!(plan.extensions[0].target_name, "Widget-Extension");
        assert!(plan.extensions[0].is_extension);
        assert_eq!(plan.package_deployment_target, Some(Version::new(17, 0, 0)));
    }

    #[test]
    fn the_app_product_must_be_unambiguous() {
        let g = graph();
        assert_eq!(g.plan(Utf8Path::new("/p"), None, &[]).unwrap_err().code(), codes::config::INVALID);
        assert_eq!(g.plan(Utf8Path::new("/p"), Some("Nope"), &[]).unwrap_err().code(), codes::config::INVALID);
        assert_eq!(
            g.plan(Utf8Path::new("/p"), Some("RussetProbe"), &["Nope".into()]).unwrap_err().code(),
            codes::config::INVALID
        );
    }

    #[test]
    fn the_builder_manifest_matches_the_design() {
        let plan = graph().plan(Utf8Path::new("/p"), Some("RussetProbe"), &["Widget".into()]).unwrap();
        let m = builder_manifest(&plan, Version::new(17, 0, 0), "../../..");
        assert!(m.starts_with("// swift-tools-version: 6.0\n// Generated by Russet. Do not edit.\n"));
        assert!(m.contains("platforms: [.iOS(\"17.0\")]"));
        assert!(m.contains("dependencies: [.package(name: \"App\", path: \"../../..\")]"));
        assert!(m.contains("name: \"RussetProbe-App\", dependencies: [.product(name: \"RussetProbe\", package: \"App\")], path: \"Sources/RussetProbe-App\""));
        assert!(m.contains("\"-Xlinker\", \"-rpath\", \"-Xlinker\", \"@executable_path/Frameworks\""));
        assert!(m.contains("\"_NSExtensionMain\""));
        assert!(m.contains("@executable_path/../../Frameworks"));
    }

    #[test]
    fn relative_paths() {
        assert_eq!(relative_path(Utf8Path::new("/p/.russet/work/builder"), Utf8Path::new("/p")), "../../..");
        assert_eq!(relative_path(Utf8Path::new("/p/.russet/work/builder"), Utf8Path::new("/p/pkg")), "../../../pkg");
        assert_eq!(relative_path(Utf8Path::new("/a"), Utf8Path::new("/a")), ".");
    }

    #[test]
    fn output_lines_become_events() {
        assert_eq!(
            parse_line(LogStream::Stdout, "[20 / 32] RussetProbe"),
            Some(BuildEvent::Progress { done: Some(20), total: Some(32), message: Some("RussetProbe".into()) })
        );
        assert_eq!(
            parse_line(LogStream::Stdout, "[31/98]"),
            Some(BuildEvent::Progress { done: Some(31), total: Some(98), message: None })
        );
        assert_eq!(
            parse_line(LogStream::Stderr, "ld64.lld: warning: x does not support linking for platform iOS"),
            None
        );
        let Some(BuildEvent::Diagnostic { diagnostic: d }) =
            parse_line(LogStream::Stderr, "/p/Sources/A/ContentView.swift:6:13: error: cannot find 'Foo' in scope")
        else {
            panic!("not a diagnostic")
        };
        assert_eq!(
            (d.file.as_deref(), d.line, d.column, d.severity),
            (Some("/p/Sources/A/ContentView.swift"), Some(6), Some(13), Severity::Error)
        );
        assert_eq!(d.message, "cannot find 'Foo' in scope");
        let d = parse_diagnostic("error: link command failed with exit code 1").unwrap();
        assert_eq!((d.file, d.severity), (None, Severity::Error));
        let d = parse_diagnostic("ld64.lld: error: undefined symbol: _foo").unwrap();
        assert_eq!(d.tool.as_deref(), Some("ld64.lld"));
        assert!(matches!(parse_line(LogStream::Stdout, "Build complete! (6.51 secs)"), Some(BuildEvent::Log { .. })));
    }

    #[test]
    fn environment_dumps_are_redacted() {
        let line = "note: running /usr/bin/env -i HOME=/home/u GITHUB_TOKEN=ghp_secret PATH=/usr/bin actool --version";
        let r = redact_env_dump(line);
        assert!(!r.contains("ghp_secret") && !r.contains("/home/u"));
        assert!(r.contains("GITHUB_TOKEN=<redacted>") && r.contains("actool --version"));
        assert_eq!(redact_env_dump("plain A=b"), "plain A=b");
    }

    #[test]
    fn the_environment_is_explicit_and_never_has_sdkroot() {
        // The caller's environment has SDKROOT and a secret; neither reaches SwiftPM, even when passed as extra.
        let caller_path = std::env::join_paths(["/usr/bin", "/bin"]).unwrap_or_default();
        let caller: BTreeMap<&str, OsString> = BTreeMap::from([
            ("SDKROOT", OsString::from("/bogus/iPhoneOS.sdk")),
            ("GITHUB_TOKEN", OsString::from("russet-canary-7f3a9c")),
            ("HOME", OsString::from("/home/u")),
            ("PATH", caller_path),
        ]);
        let env = tool_env_from(
            |k| caller.get(k).cloned(),
            &[Utf8Path::new("/linker/bin")],
            &[("XCODE_EXTRA_PLATFORM_FOLDERS", "/v"), ("SDKROOT", "/also/bogus")],
        );
        assert!(!env.contains_key(&OsString::from("SDKROOT")));
        assert!(!env.contains_key(&OsString::from("GITHUB_TOKEN")));
        assert_eq!(env.get(&OsString::from("HOME")).and_then(|v| v.to_str()), Some("/home/u"));
        assert_eq!(
            env.get(&OsString::from("PATH")),
            std::env::join_paths(["/linker/bin", "/usr/bin", "/bin"]).ok().as_ref()
        );
        assert_eq!(env.get(&OsString::from("XCODE_EXTRA_PLATFORM_FOLDERS")).and_then(|v| v.to_str()), Some("/v"));
        assert!(env.keys().all(|k| k == "PATH"
            || k == "XCODE_EXTRA_PLATFORM_FOLDERS"
            || ENV_ALLOWLIST.contains(&k.to_str().unwrap_or_default())));
        // The process-environment form never has SDKROOT either, whatever the test runner's environment holds.
        let env = tool_env(&[Utf8Path::new("/linker/bin")], &[("XCODE_EXTRA_PLATFORM_FOLDERS", "/v")]);
        assert!(!env.contains_key(&OsString::from("SDKROOT")));
    }

    #[test]
    fn the_toolchain_library_dir_is_used_only_when_present() {
        let root = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(root.path()).unwrap();
        let swift = root.join("usr/bin/swift");
        assert_eq!(toolchain_library_dir(&swift), None);
        std::fs::create_dir_all(root.join("usr/lib/swift/linux")).unwrap();
        assert_eq!(toolchain_library_dir(&swift), Some(root.join("usr/lib/swift/linux")));
        assert_eq!(toolchain_library_dir(Utf8Path::new("swift")), None);
    }

    #[test]
    fn identities_become_path_components() {
        assert_eq!(path_component("swift-argument-parser"), "swift-argument-parser");
        assert_eq!(path_component("scope.name"), "scope.name");
        assert_eq!(path_component("a/b c"), "a_b_c");
        assert_eq!(path_component(".."), "_..");
        assert_eq!(path_component(""), "_");
    }

    #[test]
    fn failures_are_classified() {
        let compile = Diagnostic {
            severity: Severity::Error,
            message: "cannot find 'Foo'".into(),
            file: Some("a.swift".into()),
            line: Some(1),
            column: Some(2),
            tool: None,
            code: None,
        };
        assert_eq!(build_failure(std::slice::from_ref(&compile), &[], None).code(), codes::build::COMPILE_FAILED);
        let link = Diagnostic {
            severity: Severity::Error,
            message: "undefined symbol: _x".into(),
            file: None,
            line: None,
            column: None,
            tool: Some("ld64.lld".into()),
            code: None,
        };
        let e = build_failure(&[link], &[], Some(Utf8Path::new("/p/.russet/logs/job_1.jsonl")));
        assert_eq!(e.code(), codes::build::LINK_FAILED);
        assert!(e.hint().is_some_and(|h| h.contains("job_1.jsonl")));
    }
}
