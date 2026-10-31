// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `toolchain` handlers (ARCHITECTURE §8.1, §16). Owner: W3 (`toolchain.pins.update`: W26).
//!
//! [`list`], [`resolve`], [`install`], [`uninstall`] and [`gc`] work on the services alone and are tested without
//! a daemon (`tests/toolchain_handlers.rs`). `toolchain.changed` comes from `Toolchains::subscribe`:
//! [`forward_changes`] sends each change to every connection through the daemon runtime's [`Notify`].

use russet_config::ConfigService;
use russet_core::error::Result;
use russet_core::progress::{LogStream, ProgressSink};
use russet_ipc::Router;
use russet_protocol::toolchain as p;
use russet_toolchain::{Component, Provenance, Requirement, SdkInfo, ToolInfo, Toolchains};
use tokio_util::sync::CancellationToken;

use super::config::{Notify, config_of, forward, project_root, toolchains_of};
use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ToolchainList, _, _>(|cx: DaemonCtx, _rq, params| async move {
        // The configuration is needed only for a project's pins.
        let config = if params.project.is_some() { Some(config_of(&cx)?) } else { None };
        list(&*toolchains_of(&cx)?, config.as_deref(), params).await
    });
    r.request::<p::ToolchainResolve, _, _>(|cx: DaemonCtx, _rq, params| async move {
        resolve(&*toolchains_of(&cx)?, &*config_of(&cx)?, params).await
    });
    r.request::<p::ToolchainInstall, _, _>(|cx: DaemonCtx, rq: russet_ipc::RequestCtx, params| async move {
        install(&*toolchains_of(&cx)?, &*config_of(&cx)?, params, &LogProgress, &rq.cancel).await
    });
    r.request::<p::ToolchainUninstall, _, _>(|cx: DaemonCtx, _rq, params| async move {
        uninstall(&*toolchains_of(&cx)?, params).await
    });
    r.request::<p::ToolchainGc, _, _>(
        |cx: DaemonCtx, _rq, params| async move { gc(&*toolchains_of(&cx)?, params).await },
    );
    r.request::<p::ToolchainPinsUpdate, _, _>(not_implemented::<p::ToolchainPinsUpdate>);
}

/// Progress of a long-running install. Until the IPC layer's `ProgressReporter` (W2, F1) forwards `$/progress`,
/// installs report through the daemon log.
pub struct LogProgress;

impl ProgressSink for LogProgress {
    fn begin(&self, title: &str, _cancellable: bool) {
        tracing::info!(target: "russetd::toolchain", "{title}");
    }
    fn report(&self, msg: Option<&str>, pct: Option<u8>) {
        tracing::debug!(target: "russetd::toolchain", message = msg.unwrap_or(""), percentage = pct);
    }
    fn end(&self, msg: Option<&str>) {
        tracing::info!(target: "russetd::toolchain", "{}", msg.unwrap_or("done"));
    }
    fn log(&self, _stream: LogStream, line: &str) {
        tracing::info!(target: "russetd::toolchain", "{line}");
    }
}

/// The wire component.
pub fn wire_component(c: Component) -> p::ToolComponent {
    match c {
        Component::Swift => p::ToolComponent::Swift,
        Component::DarwinSdk => p::ToolComponent::DarwinSdk,
        Component::Linker => p::ToolComponent::Linker,
        Component::Lldb => p::ToolComponent::Lldb,
        Component::LldbDap => p::ToolComponent::LldbDap,
        Component::LibPython => p::ToolComponent::Libpython,
        Component::Xtool => p::ToolComponent::Xtool,
        Component::Flutter => p::ToolComponent::Flutter,
        Component::GenSnapshotIos => p::ToolComponent::GenSnapshotIos,
        Component::Llvm => p::ToolComponent::Llvm,
    }
}

fn component_from_wire(c: p::ToolComponent) -> Component {
    match c {
        p::ToolComponent::Swift => Component::Swift,
        p::ToolComponent::DarwinSdk => Component::DarwinSdk,
        p::ToolComponent::Linker => Component::Linker,
        p::ToolComponent::Lldb => Component::Lldb,
        p::ToolComponent::LldbDap => Component::LldbDap,
        p::ToolComponent::Libpython => Component::LibPython,
        p::ToolComponent::Xtool => Component::Xtool,
        p::ToolComponent::Flutter => Component::Flutter,
        p::ToolComponent::GenSnapshotIos => Component::GenSnapshotIos,
        p::ToolComponent::Llvm => Component::Llvm,
    }
}

fn managed_component(c: p::ManagedComponent) -> Component {
    match c {
        p::ManagedComponent::PythonShared => Component::LibPython,
        p::ManagedComponent::DarwinTools => Component::Linker,
        p::ManagedComponent::Swift => Component::Swift,
        p::ManagedComponent::Llvm => Component::Llvm,
        p::ManagedComponent::Flutter => Component::Flutter,
        p::ManagedComponent::Xtool => Component::Xtool,
    }
}

/// The wire form of a resolved tool.
pub fn wire_tool(t: &ToolInfo) -> p::ToolInfo {
    p::ToolInfo {
        component: wire_component(t.component),
        version: t.version.clone(),
        path: t.path.to_string_lossy().into_owned(),
        provenance: match &t.provenance {
            Provenance::Managed { pin } => p::ToolProvenance::Managed { pin: pin.clone() },
            Provenance::System => p::ToolProvenance::System,
            Provenance::Uv => p::ToolProvenance::Uv,
            Provenance::XtoolBundle { bundle } => {
                p::ToolProvenance::XtoolBundle { bundle: bundle.to_string_lossy().into_owned() }
            }
            Provenance::User => p::ToolProvenance::User,
            Provenance::ProjectPin => p::ToolProvenance::ProjectPin,
        },
        env: t.env.clone(),
        ok: t.ok,
        problems: t.problems.clone(),
    }
}

/// The wire form of an SDK.
pub fn wire_sdk(s: &SdkInfo) -> p::SdkInfo {
    p::SdkInfo {
        id: s.id.clone(),
        xcode_version: s.xcode_version.clone(),
        xcode_build: s.xcode_build.clone(),
        ios_sdk: s.ios_sdk.clone(),
        swift_sdk_name: s.swift_sdk_name.clone(),
        bundle: s.bundle.to_string_lossy().into_owned(),
        darwin_tools: s.darwin_tools.clone(),
        compatible_swift: s.compatible_swift.clone(),
    }
}

/// The components a project's `[toolchain]` and `[debug]` sections choose.
const PINNABLE: [Component; 5] =
    [Component::Swift, Component::DarwinSdk, Component::Linker, Component::Lldb, Component::LldbDap];

/// `toolchain.list`: every candidate in resolution order. With a project, the candidates its pins exclude are
/// `ok: false` with the reason in `problems`, and a tool the project names by path comes first, so the first `ok`
/// candidate of each component is the one `toolchain.resolve` picks for that project. `config` is needed only
/// with a project.
pub async fn list(
    toolchains: &Toolchains,
    config: Option<&ConfigService>,
    params: p::ToolchainListParams,
) -> Result<p::ToolchainListResult> {
    let root = project_root(params.project.as_deref())?;
    let mut tools = toolchains.probe_all(params.refresh.unwrap_or(false)).await;
    if let (Some(root), Some(config)) = (root.as_deref(), config) {
        for c in PINNABLE {
            let req = requirement(config, Some(root), c)?;
            if let Some(path) = req.path() {
                let named = match toolchains.resolve(c, &req).await {
                    Ok(t) => t,
                    Err(e) => ToolInfo {
                        component: c,
                        version: String::new(),
                        path: path.into(),
                        provenance: Provenance::ProjectPin,
                        env: Default::default(),
                        ok: false,
                        problems: vec![e.message().to_owned()],
                    },
                };
                let at = tools.iter().position(|t| t.component == c).unwrap_or(tools.len());
                tools.insert(at, named);
            }
            for t in tools.iter_mut().filter(|t| t.component == c) {
                if let Some(why) = toolchains.unmet(t, &req) {
                    t.ok = false;
                    t.problems.push(why);
                }
            }
        }
    }
    let sdks = toolchains.sdks().await?;
    Ok(p::ToolchainListResult {
        tools: tools.iter().map(wire_tool).collect(),
        sdks: sdks.iter().map(wire_sdk).collect(),
        pins: p::PinsInfo { id: toolchains.pins().id.clone(), source: toolchains.pins_source().to_owned() },
    })
}

/// The requirement a project puts on a component: `[toolchain] swift`, `xcode-sdk` and `linker`, and
/// `[debug] lldb` (RD §8.1).
pub fn requirement(config: &ConfigService, project: Option<&std::path::Path>, c: Component) -> Result<Requirement> {
    let Some(root) = project else { return Ok(Requirement::any()) };
    let eff = config.effective(Some(root))?;
    let t = &eff.project.toolchain;
    // A path in the project file is relative to the project root, never to the daemon's working directory.
    let in_project = |f: &str| {
        let looks_like_path = f.contains('/') || f.contains('\\');
        if looks_like_path && !std::path::Path::new(f).is_absolute() {
            root.join(f).to_string_lossy().into_owned()
        } else {
            f.to_owned()
        }
    };
    Ok(match c {
        Component::Swift => Requirement { version: t.swift.clone(), flavor: None },
        Component::DarwinSdk => Requirement { version: t.xcode_sdk.clone(), flavor: None },
        Component::Linker => Requirement { version: None, flavor: t.linker.as_deref().map(in_project) },
        Component::Lldb | Component::LldbDap => {
            Requirement { version: None, flavor: Some(in_project(&eff.project.debug.lldb)) }
        }
        _ => Requirement::any(),
    })
}

/// `toolchain.resolve`.
pub async fn resolve(
    toolchains: &Toolchains,
    config: &ConfigService,
    params: p::ToolchainResolveParams,
) -> Result<p::ToolInfo> {
    let root = project_root(params.project.as_deref())?;
    let c = component_from_wire(params.component);
    let req = requirement(config, root.as_deref(), c)?;
    Ok(wire_tool(&toolchains.resolve(c, &req).await?))
}

/// `toolchain.install`: honours `storage.min-free-space` from `config.toml`.
pub async fn install(
    toolchains: &Toolchains,
    config: &ConfigService,
    params: p::ToolchainInstallParams,
    progress: &dyn ProgressSink,
    cancel: &CancellationToken,
) -> Result<p::ToolInfo> {
    toolchains.set_min_free_space(config.global().storage.min_free_space.bytes());
    let c = managed_component(params.component);
    let tool = toolchains.install(c, params.version.as_deref().unwrap_or(""), progress, cancel).await?;
    Ok(wire_tool(&tool))
}

/// `toolchain.uninstall`.
pub async fn uninstall(
    toolchains: &Toolchains,
    params: p::ToolchainUninstallParams,
) -> Result<p::ToolchainUninstallResult> {
    let freed = toolchains.uninstall(managed_component(params.component), &params.version).await?;
    Ok(p::ToolchainUninstallResult { freed_bytes: freed })
}

/// `toolchain.gc`.
pub async fn gc(toolchains: &Toolchains, params: p::ToolchainGcParams) -> Result<p::ToolchainGcResult> {
    let report = toolchains.gc(params.dry_run.unwrap_or(false)).await?;
    Ok(p::ToolchainGcResult {
        freed_bytes: report.freed_bytes,
        removed: report.removed.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
    })
}

/// The `toolchain.changed` params of a change the manager reported.
pub fn changed_params(components: &[Component]) -> p::ToolchainChangedParams {
    p::ToolchainChangedParams { components: components.iter().copied().map(wire_component).collect() }
}

/// Sends `toolchain.changed` through `notify` for every install, uninstall and GC of `toolchains` until
/// `shutdown`. The daemon runtime calls it once, when it starts the toolchain manager.
pub fn forward_changes(
    toolchains: &Toolchains,
    shutdown: CancellationToken,
    notify: Notify,
) -> tokio::task::JoinHandle<()> {
    forward::<p::ToolchainChanged, _>(toolchains.subscribe(), shutdown, notify, |c: &Vec<Component>| changed_params(c))
}
