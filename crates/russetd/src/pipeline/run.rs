// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The run pipeline over ports (ARCHITECTURE §17.1, RD §4.1–§4.2):
//!
//! 1. resolve the device (russet-debug's `DeviceServices`);
//! 2. build ([`ProjectBuilder`]);
//! 3. provision: a GET-only plan, applied only when it needs no Apple-side change in Wave 1 ([`Provisioning`]);
//! 4. sign a copy into `<project>/.russet/build/<profile>-iphoneos/signed/` ([`AppSigner`]); the unsigned
//!    product stays for the debugger;
//! 5. install when the install ledger says the signed app changed (or `install: always`): the pipeline reads the
//!    CDHash of the signed main executable itself ([`russet_debug::macho`]), so the ledger never depends on what
//!    a signer reports;
//! 6. launch (`run.start`) or launch suspended and pre-attach the broker (`debug.prepare`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::progress::{EventSink, LogStream};
use russet_debug::async_trait;
use russet_debug::launch::LaunchStrategy;
use russet_debug::lldb::{InitSettings, LldbPlatform};
use russet_debug::macho::MachIdentity;
use russet_debug::{
    AttachMode, BrokerEndpoint, DebugRequest, DebugService, DebugTarget, Frontend, RunRequest, SessionHandle,
    SessionLease, TerminateMode,
};
use russet_device::{DeviceSelector, Udid};
use russet_protocol::debug::{
    BrokerInfo, BrokerTransport, DebugFrontend, DebugPrepareParams, DebugPrepareResult, DebugToolComponent,
    DebugToolInfo, DebugToolProvenance, DebuggerFlavor,
};
use russet_protocol::run::{
    LaunchMode, RunListResult, RunOutputQueryParams, RunOutputQueryResult, RunStartParams, RunStartResult,
    RunStopParams, RunStopResult, StopMode,
};
use russet_toolchain::{Component, Provenance, ToolInfo};
use tokio_util::sync::CancellationToken;

use super::ledger::InstallLedger;

/// What to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSpec {
    /// The absolute project root.
    pub project: PathBuf,
    pub target: Option<String>,
    /// `debug` unless the request names another configuration.
    pub configuration: String,
    /// `false` (`russet run --no-build`): use the last build's product.
    pub build: bool,
}

/// What the build produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltProduct {
    /// The unsigned `.app`.
    pub app: PathBuf,
    /// The main executable inside it: the debugger's local file (its UUID matches the signed copy's).
    pub executable: PathBuf,
    /// The effective bundle ID (after the bundle-ID policy, ARCHITECTURE §16.5).
    pub bundle_id: String,
    /// The build profile, for `.russet/build/<profile>-iphoneos/`.
    pub profile: String,
    /// The SDK, for LLDB's `target.sdk-path`.
    pub sdk_path: Option<PathBuf>,
}

/// The build step.
#[async_trait]
pub trait ProjectBuilder: Send + Sync {
    async fn build(&self, spec: &BuildSpec, sink: &dyn EventSink, cancel: &CancellationToken) -> Result<BuiltProduct>;
}

/// What provisioning found for a build.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provisioned {
    /// A provisioning profile per bundle: relative path inside the app (`""` is the app) → `.mobileprovision`.
    pub profiles: BTreeMap<String, PathBuf>,
    pub team_id: Option<String>,
    /// Apple-side changes the plan needed; Wave 1 applies only zero-mutation plans.
    pub mutations: usize,
}

/// The provisioning step (`Provisioner::plan` GET-only, then `apply` for a zero-mutation plan).
#[async_trait]
pub trait Provisioning: Send + Sync {
    async fn provision(&self, product: &BuiltProduct, udid: &str) -> Result<Provisioned>;
}

/// A signed copy of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedApp {
    pub app: PathBuf,
}

/// The signing step.
#[async_trait]
pub trait AppSigner: Send + Sync {
    /// Signs a copy of `product.app` at `dest`, inside-out, and verifies it.
    async fn sign(&self, product: &BuiltProduct, provisioned: &Provisioned, dest: &Path) -> Result<SignedApp>;
}

/// LLDB and lldb-dap from the toolchain resolver, with their `env` (ARCHITECTURE §16.2).
#[async_trait]
pub trait LldbResolver: Send + Sync {
    async fn lldb(&self, flavor: DebuggerFlavor) -> Result<ToolInfo>;
    async fn lldb_dap(&self, flavor: DebuggerFlavor) -> Result<ToolInfo>;
}

/// The pipeline and the session queries behind `run.*` and `debug.prepare`.
#[derive(Clone)]
pub struct Pipeline {
    pub builder: Arc<dyn ProjectBuilder>,
    pub provisioning: Arc<dyn Provisioning>,
    pub signer: Arc<dyn AppSigner>,
    pub lldb: Arc<dyn LldbResolver>,
    pub debug: Arc<DebugService>,
    pub ledger: Arc<InstallLedger>,
}

/// The app, ready on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyApp {
    pub project: PathBuf,
    pub udid: Udid,
    pub product: BuiltProduct,
    pub signed: SignedApp,
    /// The signed main executable's identity: its CDHash (the ledger's key) and `LC_UUID`.
    pub identity: MachIdentity,
    pub installed: bool,
}

/// The identity of the signed main executable. `None` fields when it has no code signature (the ledger then
/// installs every time).
pub async fn signed_identity(signed: &SignedApp, product: &BuiltProduct) -> Result<MachIdentity> {
    let exe = signed.app.join(product.executable.file_name().unwrap_or_default());
    let path = exe.clone();
    let read = tokio::task::spawn_blocking(move || russet_debug::macho::identity(&path))
        .await
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("reading the signed executable failed: {e}")))?;
    let id = read.map_err(|e| {
        Error::new(codes::core::IO, format!("reading the signed executable {}: {e}", exe.display()))
            .with_hint("The signer should have written it; run again with `--install always` after a clean build.")
    })?;
    Ok(id.unwrap_or_default())
}

/// `<project>/.russet/build/<profile>-iphoneos/signed/` (ARCHITECTURE §14.8).
pub fn signed_dir(project: &Path, profile: &str) -> PathBuf {
    project.join(".russet").join("build").join(format!("{profile}-iphoneos")).join("signed")
}

/// `<project>/.russet/debug/` (`attach.lldb` goes to `<sid>/` below it).
pub fn debug_dir(project: &Path) -> PathBuf {
    project.join(".russet").join("debug")
}

fn project_root(p: &str) -> Result<PathBuf> {
    let path = PathBuf::from(p);
    if !path.is_absolute() || !path.is_dir() {
        return Err(Error::new(codes::config::PROJECT_NOT_FOUND, format!("no project at `{p}`"))
            .with_hint("Pass the absolute project root (`--project DIR`), or run from inside the project.")
            .with_details(serde_json::json!({ "project": p })));
    }
    Ok(path)
}

fn cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(Error::new(codes::core::CANCELLED, "cancelled"));
    }
    Ok(())
}

/// Reports a pipeline stage (`JobEvent` kind `stage`, ARCHITECTURE §9.2) through the sink's progress.
pub fn stage(sink: &dyn EventSink, name: &str, state: &str) {
    sink.report(Some(&format!("{name}: {state}")), None);
    sink.log(LogStream::Info, &format!("stage {name} {state}"));
}

fn strategy(m: LaunchMode) -> LaunchStrategy {
    match m {
        LaunchMode::Coredevice => LaunchStrategy::CoreDevice,
        LaunchMode::Dvt => LaunchStrategy::Dvt,
        LaunchMode::Auto | LaunchMode::None => LaunchStrategy::Auto,
    }
}

impl Pipeline {
    /// Steps 1–5: the app is built, signed and installed (or already was).
    pub async fn ready_app(
        &self,
        p: &RunStartParams,
        sink: &dyn EventSink,
        cancel: &CancellationToken,
    ) -> Result<ReadyApp> {
        let project = project_root(&p.project)?;
        stage(sink, "resolve", "started");
        let devices = self.debug.devices();
        let selector = match &p.udid {
            Some(u) => DeviceSelector::Udid(Udid(u.clone())),
            None => DeviceSelector::Only,
        };
        let udid = devices.resolve(&selector).await?;
        stage(sink, "resolve", "finished");
        cancelled(cancel)?;

        let spec = BuildSpec {
            project: project.clone(),
            target: p.target.clone(),
            configuration: p.configuration.clone().unwrap_or_else(|| "debug".to_owned()),
            build: p.build.unwrap_or(true),
        };
        let product = self.builder.build(&spec, sink, cancel).await?;
        cancelled(cancel)?;

        let provisioned = self.provisioning.provision(&product, &udid.0).await?;
        if provisioned.mutations > 0 {
            return Err(Error::new(
                codes::perm::APPROVAL_REQUIRED,
                format!("provisioning needs {} Apple-side change(s)", provisioned.mutations),
            )
            .with_hint("Review the plan with `russet provision --dry-run`; applying mutations lands with W12."));
        }
        cancelled(cancel)?;

        stage(sink, "sign", "started");
        let app_name = product.app.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "App.app".into());
        let dest = signed_dir(&project, &product.profile).join(app_name);
        let signed = self.signer.sign(&product, &provisioned, &dest).await?;
        let identity = signed_identity(&signed, &product).await?;
        if identity.cdhash.is_none() {
            sink.log(LogStream::Info, "the signed executable has no code signature Russet can read: installing");
        }
        stage(sink, "sign", "finished");
        cancelled(cancel)?;

        let mode = p.install.unwrap_or_default();
        let cdhash = identity.cdhash.as_deref();
        let installed = if self.ledger.needs_install(&udid.0, &product.bundle_id, cdhash, mode) {
            stage(sink, "install", "started");
            if let Err(e) = devices.install(&udid, &signed.app, sink).await {
                let _ = self.ledger.forget(&udid.0, &product.bundle_id);
                stage(sink, "install", "failed");
                return Err(e);
            }
            self.ledger.record(&udid.0, &product.bundle_id, cdhash)?;
            stage(sink, "install", "finished");
            true
        } else {
            sink.log(LogStream::Info, "install skipped: the installed app is current (install ledger)");
            false
        };
        Ok(ReadyApp { project, udid, product, signed, identity, installed })
    }

    fn run_request(&self, p: &RunStartParams, app: &ReadyApp) -> RunRequest {
        let mut r = RunRequest::new(DeviceSelector::Udid(app.udid.clone()), app.product.bundle_id.clone());
        r.args = p.args.clone().unwrap_or_default();
        r.env = p.env.clone().unwrap_or_default();
        r.launch = strategy(p.launch.unwrap_or_default());
        r.lease = p.session_lease.unwrap_or(SessionLease::Owned);
        r.project = Some(app.project.clone());
        r.process_name = app.product.executable.file_name().map(|n| n.to_string_lossy().into_owned());
        r.local_executable = Some(app.product.executable.clone());
        r
    }

    /// `run.start`.
    pub async fn start(
        &self,
        p: RunStartParams,
        sink: &dyn EventSink,
        cancel: &CancellationToken,
    ) -> Result<(RunStartResult, Option<SessionHandle>)> {
        let app = self.ready_app(&p, sink, cancel).await?;
        if p.launch == Some(LaunchMode::None) {
            let r = RunStartResult {
                session_id: None,
                pid: None,
                bundle_id: app.product.bundle_id,
                installed: app.installed,
            };
            return Ok((r, None));
        }
        stage(sink, "launch", "started");
        let h = self.debug.run(self.run_request(&p, &app)).await?;
        stage(sink, "launch", "finished");
        let r = RunStartResult {
            session_id: Some(h.id().clone()),
            pid: h.pid(),
            bundle_id: app.product.bundle_id,
            installed: app.installed,
        };
        Ok((r, Some(h)))
    }

    /// `debug.prepare`.
    pub async fn prepare_debug(
        &self,
        p: DebugPrepareParams,
        sink: &dyn EventSink,
        cancel: &CancellationToken,
    ) -> Result<(DebugPrepareResult, SessionHandle)> {
        let flavor = p.flavor.unwrap_or_default();
        let lldb = self.lldb.lldb(flavor).await?;
        let lldb_dap = self.lldb.lldb_dap(flavor).await.unwrap_or_else(|e| ToolInfo {
            component: Component::LldbDap,
            version: String::new(),
            path: PathBuf::new(),
            provenance: Provenance::System,
            env: BTreeMap::new(),
            ok: false,
            problems: vec![e.message().to_owned()],
        });
        let app = self.ready_app(&p.run, sink, cancel).await?;
        stage(sink, "attach", "started");
        let attach_mode = p.attach_mode.unwrap_or(AttachMode::Broker);
        let stop_on_entry = p.stop_on_entry.unwrap_or(false);
        let req = DebugRequest {
            run: self.run_request(&p.run, &app),
            stop_on_entry,
            frontend: match p.frontend.unwrap_or_default() {
                DebugFrontend::Dap => Frontend::Dap,
                DebugFrontend::LldbCli => Frontend::LldbCli,
                DebugFrontend::None => Frontend::None,
            },
            attach_mode,
            flavor,
            lldb: Some(lldb.clone()),
            lldb_dap: Some(lldb_dap.clone()),
            target: DebugTarget {
                local_executable: app.product.executable.clone(),
                local_app_dir: Some(app.signed.app.clone()),
                sdk_path: app.product.sdk_path.clone(),
                arch: Some("arm64-apple-ios".to_owned()),
            },
            platform: LldbPlatform::RemoteIos { sysroot: None },
            init: InitSettings::default(),
            script_root: Some(debug_dir(&app.project)),
        };
        let prepared = self.debug.prepare_debug(req).await?;
        stage(sink, "attach", "finished");
        let dap_config = serde_json::json!({
            "type": "lldb-dap",
            "request": "attach",
            "name": format!("iPhone (Russet) - {}", app.product.bundle_id),
            "initCommands": prepared.lldb.init_commands,
            "attachCommands": prepared.dap_attach_commands,
            "stopOnEntry": stop_on_entry,
            "terminateCommands": [],
            "sourceMap": [],
            "timeout": 60,
        });
        let result = DebugPrepareResult {
            session_id: prepared.session.id().clone(),
            pid: prepared.pid,
            bundle_id: app.product.bundle_id.clone(),
            lldb: wire_tool(&lldb),
            lldb_dap: wire_tool(&lldb_dap),
            broker: broker_info(&prepared.broker, &prepared.lldb_url),
            init_commands: prepared.lldb.init_commands.clone(),
            attach_commands: prepared.lldb.attach_commands.clone(),
            dap_config,
            dap: None,
            attach_script: prepared.attach_script.as_ref().map(|p| p.to_string_lossy().into_owned()),
            attach_mode,
        };
        Ok((result, prepared.session))
    }

    /// `run.list`.
    pub fn list(&self) -> RunListResult {
        RunListResult { sessions: self.debug.sessions() }
    }

    /// `run.output.query`.
    pub fn output_query(&self, q: &RunOutputQueryParams) -> Result<RunOutputQueryResult> {
        let h = self
            .debug
            .session(&q.session_id)
            .ok_or_else(|| russet_debug::errors::session_not_found(q.session_id.as_str()))?;
        let filter = q.filter.clone().unwrap_or_default();
        let page = h.logs().read(q.after_seq, q.since_ms, q.limit.map(|l| l as usize), &filter);
        Ok(RunOutputQueryResult { entries: page.entries, dropped: page.dropped, next_seq: page.next_seq })
    }

    /// `run.stop`.
    pub async fn stop(&self, q: &RunStopParams) -> Result<RunStopResult> {
        let h = self
            .debug
            .session(&q.session_id)
            .ok_or_else(|| russet_debug::errors::session_not_found(q.session_id.as_str()))?;
        let mode = match q.mode.unwrap_or_default() {
            StopMode::Kill => TerminateMode::Kill,
            StopMode::Detach => TerminateMode::Detach,
        };
        h.terminate(mode).await?;
        Ok(RunStopResult {})
    }
}

/// `toolchain.resolve`'s `ToolInfo` on the wire (the shape of ARCHITECTURE §8.1).
pub fn wire_tool(t: &ToolInfo) -> DebugToolInfo {
    DebugToolInfo {
        component: match t.component {
            Component::Lldb => DebugToolComponent::Lldb,
            Component::LldbDap => DebugToolComponent::LldbDap,
            Component::LibPython => DebugToolComponent::Libpython,
            Component::Swift => DebugToolComponent::Swift,
            Component::DarwinSdk => DebugToolComponent::DarwinSdk,
            Component::Linker => DebugToolComponent::Linker,
            Component::Xtool => DebugToolComponent::Xtool,
            Component::Flutter => DebugToolComponent::Flutter,
            Component::GenSnapshotIos => DebugToolComponent::GenSnapshotIos,
            Component::Llvm => DebugToolComponent::Llvm,
        },
        version: t.version.clone(),
        path: t.path.to_string_lossy().into_owned(),
        provenance: match &t.provenance {
            Provenance::Managed { pin } => DebugToolProvenance::Managed { pin: pin.clone() },
            Provenance::System => DebugToolProvenance::System,
            Provenance::Uv => DebugToolProvenance::Uv,
            Provenance::XtoolBundle { bundle } => {
                DebugToolProvenance::XtoolBundle { bundle: bundle.to_string_lossy().into_owned() }
            }
            Provenance::User => DebugToolProvenance::User,
            Provenance::ProjectPin => DebugToolProvenance::ProjectPin,
        },
        env: t.env.clone(),
        ok: t.ok,
        problems: t.problems.clone(),
    }
}

/// The broker endpoint on the wire.
pub fn broker_info(e: &BrokerEndpoint, lldb_url: &str) -> BrokerInfo {
    match e {
        BrokerEndpoint::Unix { path } => BrokerInfo {
            transport: BrokerTransport::Unix,
            path: Some(path.to_string_lossy().into_owned()),
            port: None,
            lldb_url: lldb_url.to_owned(),
        },
        BrokerEndpoint::Tcp { addr } => BrokerInfo {
            transport: BrokerTransport::Tcp,
            path: None,
            port: Some(addr.port()),
            lldb_url: lldb_url.to_owned(),
        },
    }
}

/// An [`EventSink`] that only traces (until the job's sink from W2's JobManager is wired in).
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingSink;

impl russet_core::progress::ProgressSink for TracingSink {
    fn begin(&self, title: &str, _cancellable: bool) {
        tracing::info!(title, "pipeline");
    }
    fn report(&self, msg: Option<&str>, pct: Option<u8>) {
        tracing::info!(msg, pct, "pipeline");
    }
    fn end(&self, msg: Option<&str>) {
        tracing::info!(msg, "pipeline done");
    }
    fn log(&self, stream: LogStream, line: &str) {
        tracing::debug!(?stream, line, "pipeline");
    }
}

impl EventSink for TracingSink {
    fn emit(&self, e: russet_core::progress::JobEventKind) {
        tracing::debug!(?e, "pipeline event");
    }
}
