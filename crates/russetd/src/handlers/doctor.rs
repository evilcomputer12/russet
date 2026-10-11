// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `doctor` handlers (ARCHITECTURE §8.1; core §11) and the configuration, secrets and toolchain checks. Owner: W3.
//!
//! russet-config, russet-secrets and russet-toolchain are layer L2 and may not depend on russet-doctor
//! (ARCHITECTURE §5.2), so their checks live here; the doctor seam (`doctor_registry.rs`) registers them [D 100].
//! They reach the daemon's services through `CheckCx` extensions, which [`run`] inserts: `Arc<ConfigService>`,
//! [`SecretsHandle`] and `Arc<Toolchains>`. A check whose service is missing reports `skip`.

use std::path::Path;
use std::sync::Arc;

use russet_config::ConfigService;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result, codes};
use russet_doctor::{Category, Check, CheckCx, CheckResult, Filter, Fix, Registry, Requires, Status, async_trait};
use russet_ipc::Router;
use russet_protocol::doctor as p;
use russet_secrets::SecretStore;
use russet_toolchain::{Component, Provenance, Toolchains};
use tokio_util::sync::CancellationToken;

use super::config::project_root;
use super::toolchain::{LogProgress, requirement};
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::DoctorRun, _, _>(|cx: DaemonCtx, _rq, params| async move {
        let services = DoctorServices::from_env(&cx.dirs, cx.config.clone(), cx.secrets.clone(), cx.toolchains.clone());
        run(&crate::doctor_registry::registry(), &services, params).await
    });
}

/// The secret store as a `CheckCx` extension (a trait object cannot be one directly).
pub struct SecretsHandle(pub Arc<dyn SecretStore>);

/// What `doctor.run` hands the checks.
pub struct DoctorServices {
    pub dirs: RussetDirs,
    pub config: Arc<ConfigService>,
    pub secrets: Arc<dyn SecretStore>,
    pub toolchains: Arc<Toolchains>,
    /// The daemon's device backend (`RUSSET_DEVICE_BACKEND`, default `live`).
    pub device_backend: String,
    /// Whether this development build refuses the `live` backend (no `RUSSET_LANE=A`, ARCHITECTURE §3.3 item 7).
    pub live_refused: bool,
    /// `USBMUXD_SOCKET_ADDRESS`.
    pub usbmux_address: Option<String>,
}

impl DoctorServices {
    /// The services, with the device backend and mux address of this process's environment (the daemon reads
    /// `RUSSET_DEVICE_BACKEND` when it is spawned, ARCHITECTURE §14.9).
    pub fn from_env(
        dirs: &RussetDirs,
        config: Arc<ConfigService>,
        secrets: Arc<dyn SecretStore>,
        toolchains: Arc<Toolchains>,
    ) -> Self {
        let var = |v: &str| std::env::var(v).ok().filter(|s| !s.is_empty());
        let device_backend = var("RUSSET_DEVICE_BACKEND").unwrap_or_else(|| "live".into());
        let live_refused =
            device_backend == "live" && cfg!(debug_assertions) && var("RUSSET_LANE").as_deref() != Some("A");
        Self {
            dirs: dirs.clone(),
            config,
            secrets,
            toolchains,
            device_backend,
            live_refused,
            usbmux_address: var("USBMUXD_SOCKET_ADDRESS"),
        }
    }
}

fn wire_category(c: Category) -> p::DoctorCategory {
    match c {
        Category::Host => p::DoctorCategory::Host,
        Category::Daemon => p::DoctorCategory::Daemon,
        Category::Config => p::DoctorCategory::Config,
        Category::Secrets => p::DoctorCategory::Secrets,
        Category::Usbmux => p::DoctorCategory::Usbmux,
        Category::Device => p::DoctorCategory::Device,
        Category::Toolchain => p::DoctorCategory::Toolchain,
        Category::Account => p::DoctorCategory::Account,
        Category::Network => p::DoctorCategory::Network,
        Category::Platform => p::DoctorCategory::Platform,
    }
}

fn category_from_wire(c: p::DoctorCategory) -> Category {
    match c {
        p::DoctorCategory::Host => Category::Host,
        p::DoctorCategory::Daemon => Category::Daemon,
        p::DoctorCategory::Config => Category::Config,
        p::DoctorCategory::Secrets => Category::Secrets,
        p::DoctorCategory::Usbmux => Category::Usbmux,
        p::DoctorCategory::Device => Category::Device,
        p::DoctorCategory::Toolchain => Category::Toolchain,
        p::DoctorCategory::Account => Category::Account,
        p::DoctorCategory::Network => Category::Network,
        p::DoctorCategory::Platform => Category::Platform,
    }
}

fn wire_status(s: Status) -> p::DoctorStatus {
    match s {
        Status::Pass => p::DoctorStatus::Pass,
        Status::Warn => p::DoctorStatus::Warn,
        Status::Fail => p::DoctorStatus::Fail,
        Status::Skip => p::DoctorStatus::Skip,
    }
}

/// `doctor.run` over `registry`.
pub async fn run(registry: &Registry, s: &DoctorServices, params: p::DoctorRunParams) -> Result<p::DoctorReport> {
    let project = project_root(params.project.as_deref())?;
    let mut cx = CheckCx {
        project,
        udid: params.udid.clone(),
        online: params.online.unwrap_or(false),
        dirs: Some(s.dirs.clone()),
        device_backend: Some(s.device_backend.clone()),
        live_refused: s.live_refused,
        usbmux_address: s.usbmux_address.clone(),
        min_free_space: Some(s.config.global().storage.min_free_space.bytes()),
        extensions: Default::default(),
    };
    cx.insert(s.config.clone());
    cx.insert(Arc::new(SecretsHandle(s.secrets.clone())));
    cx.insert(s.toolchains.clone());
    let filter = Filter {
        checks: params.checks.clone().unwrap_or_default(),
        categories: params.categories.clone().unwrap_or_default().into_iter().map(category_from_wire).collect(),
        fix: params.fix.unwrap_or(false),
    };
    let report = registry.run(&cx, &filter).await;
    let summary = report.summary();
    let host = russet_doctor::host::host_info();
    Ok(p::DoctorReport {
        russet: p::DoctorBuild {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            build_id: env!("CARGO_PKG_VERSION").to_owned(),
        },
        host: p::DoctorHost { os: host.os, distro: host.distro, arch: host.arch, wsl: host.wsl },
        checks: report
            .checks
            .into_iter()
            .map(|c| p::DoctorCheck {
                id: c.id,
                category: wire_category(c.category),
                title: c.title,
                status: wire_status(c.result.status),
                message: c.result.summary,
                detail: c.result.detail,
                hint: c.result.hint,
                fix: c.fix.and_then(|f| match f {
                    Fix::None => None,
                    Fix::Auto => Some(p::DoctorFix::Auto { applied: c.fix_applied }),
                    Fix::Command(command) => Some(p::DoctorFix::Command { command }),
                    Fix::Manual => Some(p::DoctorFix::Manual),
                }),
                duration_ms: c.duration_ms,
            })
            .collect(),
        summary: p::DoctorSummary { pass: summary.pass, warn: summary.warn, fail: summary.fail, skip: summary.skip },
    })
}

fn missing_service(what: &str) -> CheckResult {
    CheckResult::skip(format!("{what} is not available in this context"))
}

/// A check over one daemon service.
macro_rules! check {
    ($ty:ident, $id:literal, $title:literal, $cat:expr, $req:expr, $fix:expr) => {
        struct $ty;
        impl $ty {
            const ID: &'static str = $id;
        }
        #[async_trait]
        impl Check for $ty {
            fn id(&self) -> &'static str {
                Self::ID
            }
            fn title(&self) -> &'static str {
                $title
            }
            fn category(&self) -> Category {
                $cat
            }
            fn requires(&self) -> Requires {
                $req
            }
            async fn run(&self, cx: &CheckCx) -> CheckResult {
                $ty::check(cx).await
            }
            async fn fix(&self, cx: &CheckCx) -> Result<()> {
                $ty::apply_fix(cx).await
            }
            fn fix_kind(&self) -> Fix {
                $fix
            }
        }
    };
}

check!(ConfigGlobal, "config.global", "Global configuration files", Category::Config, Requires::default(), Fix::Manual);
check!(
    ConfigProject,
    "config.project",
    "Project configuration (russet.toml, russet.local.toml)",
    Category::Config,
    Requires { project: true, ..Requires::default() },
    Fix::Manual
);
check!(SecretsBackend, "secrets.backend", "Secret store", Category::Secrets, Requires::default(), Fix::Manual);
check!(ToolchainSwift, "toolchain.swift", "Swift toolchain", Category::Toolchain, Requires::default(), Fix::Manual);
check!(ToolchainSdk, "toolchain.sdk", "Darwin SDK", Category::Toolchain, Requires::default(), Fix::Manual);
check!(
    ToolchainLinker,
    "toolchain.linker",
    "Linker for the Xcode 27 SDK",
    Category::Toolchain,
    Requires::default(),
    Fix::Auto
);
check!(ToolchainLldb, "toolchain.lldb", "LLDB", Category::Toolchain, Requires::default(), Fix::Auto);
check!(ToolchainLldbDap, "toolchain.lldb-dap", "lldb-dap", Category::Toolchain, Requires::default(), Fix::Auto);
check!(ToolchainXtool, "toolchain.xtool", "xtool (optional)", Category::Toolchain, Requires::default(), Fix::None);

fn no_fix() -> Result<()> {
    Err(Error::new(codes::core::NOT_IMPLEMENTED, "no automatic fix").with_hint("Follow the check's hint."))
}

impl ConfigGlobal {
    async fn check(cx: &CheckCx) -> CheckResult {
        let Some(config) = cx.extension::<ConfigService>() else { return missing_service("the configuration") };
        let global = match config.global_loaded() {
            Ok(g) => g,
            Err(e) => return CheckResult::from_error(Status::Fail, &e),
        };
        if let Err(e) = config.mcp_policy() {
            return CheckResult::from_error(Status::Fail, &e);
        }
        if let Err(e) = config.provision_policy() {
            return CheckResult::from_error(Status::Fail, &e);
        }
        match global {
            Some(g) => CheckResult::pass(format!(
                "{} and the policy files are valid",
                g.source.path.as_deref().map_or_else(|| "config.toml".into(), |p| p.display().to_string())
            )),
            None => CheckResult::pass("no config.toml; the built-in defaults apply"),
        }
    }
    async fn apply_fix(_cx: &CheckCx) -> Result<()> {
        no_fix()
    }
}

impl ConfigProject {
    async fn check(cx: &CheckCx) -> CheckResult {
        let Some(config) = cx.extension::<ConfigService>() else { return missing_service("the configuration") };
        let Some(root) = cx.project.as_deref() else { return CheckResult::skip("no project") };
        match config.project(root) {
            Ok(p) if p.committed.is_none() && p.local.is_none() => {
                CheckResult::pass(format!("{} has no russet.toml; the defaults apply", root.display()))
            }
            Ok(p) => {
                let files: Vec<String> =
                    p.layers().filter_map(|l| l.source.path.as_ref()).map(|p| p.display().to_string()).collect();
                CheckResult::pass(format!("{} valid", files.join(" and ")))
            }
            Err(e) => CheckResult::from_error(Status::Fail, &e),
        }
    }
    async fn apply_fix(_cx: &CheckCx) -> Result<()> {
        no_fix()
    }
}

impl SecretsBackend {
    async fn check(cx: &CheckCx) -> CheckResult {
        let Some(h) = cx.extension::<SecretsHandle>() else { return missing_service("the secret store") };
        let info = h.0.status().await;
        let backend = info.kind.as_str();
        let entry = info.keyring_entry.map(|e| format!(", entry {e}")).unwrap_or_default();
        let reason = info.reason.map(|r| format!(": {r}")).unwrap_or_default();
        if !info.available {
            CheckResult::fail(format!("the {backend} backend is not available{reason}")).with_hint(
                "Start a Secret Service (KDE Wallet or GNOME Keyring), or set RUSSET_SECRETS_BACKEND=passphrase and \
                 RUSSET_SECRETS_PASSPHRASE.",
            )
        } else if info.locked {
            CheckResult::fail(format!("the {backend} backend is locked{reason}"))
                .with_hint("Unlock the keyring, or set RUSSET_SECRETS_PASSPHRASE for the passphrase store.")
        } else {
            CheckResult::pass(format!("{backend} backend available{entry}"))
        }
    }
    async fn apply_fix(_cx: &CheckCx) -> Result<()> {
        no_fix()
    }
}

fn provenance_label(p: &Provenance) -> String {
    match p {
        Provenance::Managed { pin } => format!("managed {pin}"),
        Provenance::System => "system".into(),
        Provenance::Uv => "uv".into(),
        Provenance::XtoolBundle { bundle } => format!("bundle {}", bundle.display()),
        Provenance::User => "user".into(),
        Provenance::ProjectPin => "project pin".into(),
    }
}

async fn tool_requirement(
    cx: &CheckCx,
    c: Component,
) -> std::result::Result<russet_toolchain::Requirement, CheckResult> {
    let Some(config) = cx.extension::<ConfigService>() else { return Ok(russet_toolchain::Requirement::any()) };
    requirement(&config, cx.project.as_deref(), c).map_err(|e| CheckResult::from_error(Status::Fail, &e))
}

impl ToolchainSwift {
    async fn check(cx: &CheckCx) -> CheckResult {
        let Some(tc) = cx.extension::<Toolchains>() else { return missing_service("the toolchain manager") };
        let req = match tool_requirement(cx, Component::Swift).await {
            Ok(r) => r,
            Err(r) => return r,
        };
        let swift = match tc.resolve(Component::Swift, &req).await {
            Ok(s) => s,
            Err(e) => return CheckResult::from_error(Status::Fail, &e),
        };
        let msg =
            format!("Swift {} at {} ({})", swift.version, swift.path.display(), provenance_label(&swift.provenance));
        let sdks = tc.sdks().await.unwrap_or_default();
        if let Some(sdk) = sdks.iter().find(|s| !s.compatible_swift.is_empty())
            && !swift.version.starts_with(&sdk.compatible_swift)
        {
            return CheckResult::fail(format!("{msg}; the {} SDK needs Swift {}", sdk.id, sdk.compatible_swift))
                .with_hint(format!("Install Swift {} for the Xcode {} SDK.", sdk.compatible_swift, sdk.xcode_version));
        }
        CheckResult::pass(msg)
    }
    async fn apply_fix(_cx: &CheckCx) -> Result<()> {
        no_fix()
    }
}

impl ToolchainSdk {
    async fn check(cx: &CheckCx) -> CheckResult {
        let Some(tc) = cx.extension::<Toolchains>() else { return missing_service("the toolchain manager") };
        match tc.sdks().await {
            Ok(sdks) if sdks.is_empty() => CheckResult::fail("no Darwin SDK is imported").with_hint(
                "Import the Xcode 27 SDK: `russet sdk import <Xcode_27.xip>` (W16b), or use xtool's bundle.",
            ),
            Ok(sdks) => {
                let list: Vec<String> = sdks
                    .iter()
                    .map(|s| {
                        format!(
                            "{} (iOS {}, Swift {}, darwin-tools {})",
                            s.id,
                            s.ios_sdk,
                            if s.compatible_swift.is_empty() { "?" } else { &s.compatible_swift },
                            s.darwin_tools.as_deref().unwrap_or("none")
                        )
                    })
                    .collect();
                CheckResult::pass(list.join("; "))
            }
            Err(e) => CheckResult::from_error(Status::Fail, &e),
        }
    }
    async fn apply_fix(_cx: &CheckCx) -> Result<()> {
        no_fix()
    }
}

impl ToolchainLinker {
    async fn check(cx: &CheckCx) -> CheckResult {
        let Some(tc) = cx.extension::<Toolchains>() else { return missing_service("the toolchain manager") };
        let req = match tool_requirement(cx, Component::Linker).await {
            Ok(r) => r,
            Err(r) => return r,
        };
        let linker = match tc.resolve(Component::Linker, &req).await {
            Ok(l) => l,
            Err(e) => {
                return CheckResult::from_error(Status::Fail, &e)
                    .with_hint("Run `russet doctor --fix --check toolchain.linker` to install darwin-tools 1.1.0.");
            }
        };
        let source = match &linker.provenance {
            Provenance::XtoolBundle { bundle } => {
                let dt = tc
                    .sdks()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .find(|s| &s.bundle == bundle)
                    .and_then(|s| s.darwin_tools);
                format!(
                    "darwin-tools {} ld64.lld (LLD {}) from the SDK bundle",
                    dt.as_deref().unwrap_or("?"),
                    linker.version
                )
            }
            Provenance::Managed { pin } => format!("{pin} ld64.lld (LLD {})", linker.version),
            other => format!("ld64.lld (LLD {}, {})", linker.version, provenance_label(other)),
        };
        let rejected: Vec<String> = tc
            .probe_all(false)
            .await
            .into_iter()
            .filter(|t| t.component == Component::Linker && !t.ok)
            .map(|t| format!("{} ld64.lld {} is too old", provenance_label(&t.provenance), t.version))
            .collect();
        let mut msg = source;
        if !rejected.is_empty() {
            msg = format!("{msg}; {}", rejected.join("; "));
        }
        CheckResult::pass(msg)
    }
    async fn apply_fix(cx: &CheckCx) -> Result<()> {
        install(cx, Component::Linker).await
    }
}

async fn install(cx: &CheckCx, c: Component) -> Result<()> {
    let tc = cx.extension::<Toolchains>().ok_or_else(|| {
        Error::new(codes::core::INTERNAL, "the toolchain manager is not available").with_hint("Report this bug.")
    })?;
    if let Some(min) = cx.min_free_space {
        tc.set_min_free_space(min);
    }
    tc.install(c, "", &LogProgress, &CancellationToken::new()).await.map(drop)
}

async fn lldb_check(cx: &CheckCx, c: Component) -> CheckResult {
    let Some(tc) = cx.extension::<Toolchains>() else { return missing_service("the toolchain manager") };
    let req = match tool_requirement(cx, c).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    match tc.resolve(c, &req).await {
        Ok(t) => {
            let flavor = if t.env.contains_key("PYTHONHOME") || t.env.contains_key("LD_LIBRARY_PATH") {
                "swift"
            } else {
                "llvm"
            };
            let python = match t.env.get("PYTHONHOME") {
                Some(home) => {
                    let py = tc.resolve(Component::LibPython, &russet_toolchain::Requirement::any()).await.ok();
                    match py.filter(|p| p.path.starts_with(Path::new(home))) {
                        Some(p) => format!(", python: pbs {} ({})", p.version, provenance_label(&p.provenance)),
                        None => format!(", python: {home}"),
                    }
                }
                None => String::new(),
            };
            CheckResult::pass(format!("{flavor} {} {} at {}{python}", c.as_str(), t.version, t.path.display()))
        }
        Err(e) => {
            let mut r = CheckResult::from_error(Status::Fail, &e);
            if e.code() == codes::toolchain::BROKEN {
                r = r.with_hint("Run `russet doctor --fix --check toolchain.lldb` to install python-shared (34 MB).");
            }
            r
        }
    }
}

impl ToolchainLldb {
    async fn check(cx: &CheckCx) -> CheckResult {
        lldb_check(cx, Component::Lldb).await
    }
    async fn apply_fix(cx: &CheckCx) -> Result<()> {
        install(cx, Component::LibPython).await
    }
}

impl ToolchainLldbDap {
    async fn check(cx: &CheckCx) -> CheckResult {
        lldb_check(cx, Component::LldbDap).await
    }
    async fn apply_fix(cx: &CheckCx) -> Result<()> {
        install(cx, Component::LibPython).await
    }
}

impl ToolchainXtool {
    async fn check(cx: &CheckCx) -> CheckResult {
        let Some(tc) = cx.extension::<Toolchains>() else { return missing_service("the toolchain manager") };
        match tc.resolve(Component::Xtool, &russet_toolchain::Requirement::any()).await {
            Ok(t) => CheckResult::pass(format!("xtool {} at {}", t.version, t.path.display())),
            Err(e) if e.code() == codes::toolchain::MISSING => {
                CheckResult::skip("xtool is not installed (optional: Russet builds and imports SDKs without it)")
            }
            Err(e) => CheckResult::from_error(Status::Warn, &e),
        }
    }
    async fn apply_fix(_cx: &CheckCx) -> Result<()> {
        no_fix()
    }
}

/// The configuration, secrets and toolchain checks (CORE-12). russet-config, russet-secrets and russet-toolchain
/// are layer L2 and may not depend on russet-doctor (ARCHITECTURE §5.2), so W3 implements their checks here; the
/// doctor seam (`doctor_registry.rs`) registers them.
pub fn checks() -> Vec<std::sync::Arc<dyn russet_doctor::Check>> {
    vec![
        Arc::new(ConfigGlobal),
        Arc::new(ConfigProject),
        Arc::new(SecretsBackend),
        Arc::new(ToolchainSwift),
        Arc::new(ToolchainSdk),
        Arc::new(ToolchainLinker),
        Arc::new(ToolchainLldb),
        Arc::new(ToolchainLldbDap),
        Arc::new(ToolchainXtool),
    ]
}
