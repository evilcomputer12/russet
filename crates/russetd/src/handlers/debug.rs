// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `debug` handlers (ARCHITECTURE §8.5). Owner: W7 (`debug.prepare`, Wave 1), W11 (Wave 2: attach, control,
//! inspect, evaluate, breakpoints, symbols), W18 (`debug.jitStats`, Wave 3).
//!
//! `debug.prepare` runs the pipeline up to a suspended launch with the broker attached (ARCHITECTURE §17.1).
//! The execution-control and inspection methods drive the session's DAP hub through an in-process agent client
//! (ARCHITECTURE §10.2); the symbol methods front `russet_debug::symbols::SymbolCache`.

use std::path::PathBuf;
use std::sync::Arc;

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_debug::symbols::SymbolCache;
use russet_debug::{AttachRequest, AttachTarget, Frontend};
use russet_device::{DeviceSelector, Udid};
use russet_ipc::{RequestCtx, Router};
use russet_protocol::common::SessionLease;
use russet_protocol::debug::{self as p, DebugFrontend};

use super::run::bind_owner;
use crate::DaemonCtx;
use crate::pipeline::live::pipeline_for;
use crate::pipeline::run::TracingSink;
use crate::pipeline::run::{broker_info, debug_dir};

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    crate::ctx::provide::<russet_debug::DebugService>(r, |env| {
        let services: Arc<dyn russet_debug::DeviceServices> = Arc::new(env.devices.clone());
        Ok(Arc::new(russet_debug::DebugService::new(services, russet_debug::DebugConfig::from_dirs(&env.dirs))))
    });
    r.request::<p::DebugPrepare, _, _>(prepare);
    r.request::<p::DebugAttach, _, _>(attach);
    r.request::<p::DebugControl, _, _>(control);
    r.request::<p::DebugInspect, _, _>(inspect);
    r.request::<p::DebugEvaluate, _, _>(evaluate);
    r.request::<p::DebugBreakpointSet, _, _>(breakpoint_set);
    r.request::<p::DebugBreakpointRemove, _, _>(breakpoint_remove);
    r.request::<p::DebugJitStats, _, _>(super::not_implemented::<p::DebugJitStats>);
    r.request::<p::DebugSymbolsStatus, _, _>(symbols_status);
    r.request::<p::DebugSymbolsPlan, _, _>(symbols_plan);
    r.request::<p::DebugSymbolsFetch, _, _>(symbols_fetch);
    r.request::<p::DebugSymbolsPrune, _, _>(symbols_prune);
}

async fn prepare(cx: DaemonCtx, rq: RequestCtx, params: p::DebugPrepareParams) -> Result<p::DebugPrepareResult> {
    let pipeline = pipeline_for(&cx)?;
    let (result, session) = pipeline.prepare_debug(params, &TracingSink, &rq.cancel).await?;
    bind_owner(&cx, &rq, &session);
    Ok(result)
}

async fn attach(cx: DaemonCtx, rq: RequestCtx, params: p::DebugAttachParams) -> Result<p::DebugAttachResult> {
    let device = match &params.udid {
        Some(u) => DeviceSelector::Udid(Udid(u.clone())),
        None => DeviceSelector::Only,
    };
    let target = match (params.pid, &params.bundle_id) {
        (Some(pid), _) => AttachTarget::Pid(pid),
        (None, Some(b)) => AttachTarget::Bundle(b.clone()),
        (None, None) => {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "pass --pid or --bundle to attach")
                .with_hint("`russet debug --attach --pid N` or `--bundle <id>`."));
        }
    };
    let lldb = pipeline_for(&cx)?.lldb.lldb(russet_protocol::debug::DebuggerFlavor::Swift).await.ok();
    let project = params.project.as_ref().map(PathBuf::from);
    let req = AttachRequest {
        device,
        target,
        frontend: map_frontend(params.frontend.unwrap_or_default()),
        lease: params.session_lease.unwrap_or(SessionLease::Owned),
        debug_target: None,
        lldb,
        platform: russet_debug::lldb::LldbPlatform::RemoteIos { sysroot: None },
        init: russet_debug::lldb::InitSettings::default(),
        script_root: project.as_deref().map(debug_dir),
    };
    let prepared = cx.debug.get()?.attach(req).await?;
    bind_owner(&cx, &rq, &prepared.session);
    Ok(prepared_result(&prepared))
}

fn map_frontend(f: DebugFrontend) -> Frontend {
    match f {
        DebugFrontend::Dap => Frontend::Dap,
        DebugFrontend::LldbCli => Frontend::LldbCli,
        DebugFrontend::None => Frontend::None,
    }
}

/// Maps a [`russet_debug::PreparedDebug`] (from attach) to the `PreparedDebug` wire result.
fn prepared_result(p: &russet_debug::PreparedDebug) -> p::DebugPrepareResult {
    let dap = p.dap.as_ref().map(|d| match d {
        russet_debug::DapEndpoint::Unix { path } => {
            p::DapInfo { transport: p::DapTransport::Unix, path: path.to_string_lossy().into_owned() }
        }
        russet_debug::DapEndpoint::NamedPipe { name } => {
            p::DapInfo { transport: p::DapTransport::Pipe, path: name.clone() }
        }
    });
    let tool = |info: &russet_debug::LldbLaunchPlan| p::DebugToolInfo {
        component: p::DebugToolComponent::Lldb,
        version: String::new(),
        path: info.program.to_string_lossy().into_owned(),
        provenance: p::DebugToolProvenance::System,
        env: info.env.clone(),
        ok: true,
        problems: vec![],
    };
    p::DebugPrepareResult {
        session_id: p.session.id().clone(),
        pid: p.pid,
        bundle_id: p.session.bundle_id().to_owned(),
        lldb: tool(&p.lldb),
        lldb_dap: p::DebugToolInfo {
            component: p::DebugToolComponent::LldbDap,
            version: String::new(),
            path: String::new(),
            provenance: p::DebugToolProvenance::System,
            env: Default::default(),
            ok: p.dap.is_some(),
            problems: vec![],
        },
        broker: broker_info(&p.broker, &p.lldb_url),
        init_commands: p.lldb.init_commands.clone(),
        attach_commands: p.lldb.attach_commands.clone(),
        dap_config: serde_json::json!({
            "type": "lldb-dap", "request": "attach",
            "initCommands": p.lldb.init_commands, "attachCommands": p.dap_attach_commands,
        }),
        dap,
        attach_script: p.attach_script.as_ref().map(|s| s.to_string_lossy().into_owned()),
        attach_mode: p.attach_mode,
    }
}

async fn control(cx: DaemonCtx, _rq: RequestCtx, params: p::DebugControlParams) -> Result<p::DebugControlResult> {
    cx.debug.get()?.control(params.session_id.as_str(), params.action, params.thread_id, params.wait_for_stop_ms).await
}

async fn inspect(cx: DaemonCtx, _rq: RequestCtx, params: p::DebugInspectParams) -> Result<p::DebugInspectResult> {
    cx.debug
        .get()?
        .inspect(
            params.session_id.as_str(),
            params.what,
            params.thread_id,
            params.frame_id,
            params.scope.as_deref(),
            params.depth,
        )
        .await
}

async fn evaluate(cx: DaemonCtx, _rq: RequestCtx, params: p::DebugEvaluateParams) -> Result<p::DebugEvaluateResult> {
    let context = match params.context.unwrap_or_default() {
        p::EvalContext::Repl => "repl",
        p::EvalContext::Watch => "watch",
    };
    cx.debug.get()?.evaluate(params.session_id.as_str(), &params.expression, params.frame_id, context).await
}

async fn breakpoint_set(
    cx: DaemonCtx,
    _rq: RequestCtx,
    params: p::DebugBreakpointSetParams,
) -> Result<p::DebugBreakpointSetResult> {
    let session_id = params.session_id.as_ref().map(|s| s.as_str().to_owned()).ok_or_else(|| {
        Error::new(codes::core::INVALID_ARGUMENT, "sessionId is required here")
            .with_hint("Pending source breakpoints across sessions go through the IDE (`ide.breakpoints.add`).")
    })?;
    cx.debug.get()?.breakpoint_set(&session_id, &params.file, params.line, params.condition, params.log_message).await
}

async fn breakpoint_remove(
    cx: DaemonCtx,
    _rq: RequestCtx,
    params: p::DebugBreakpointRemoveParams,
) -> Result<p::DebugBreakpointRemoveResult> {
    cx.debug.get()?.breakpoint_remove(params.breakpoint_id).await?;
    Ok(p::DebugBreakpointRemoveResult {})
}

/// Resolves a device UDID from an optional selector.
async fn resolve_udid(cx: &DaemonCtx, udid: Option<&str>) -> Result<Udid> {
    let sel = match udid {
        Some(u) => DeviceSelector::Udid(Udid(u.to_owned())),
        None => DeviceSelector::Only,
    };
    cx.debug.get()?.devices().resolve(&sel).await
}

async fn symbols_status(
    cx: DaemonCtx,
    _rq: RequestCtx,
    params: p::DebugSymbolsStatusParams,
) -> Result<p::DebugSymbolsStatusResult> {
    let cache = SymbolCache::new(&cx.dirs.cache);
    let build = match resolve_udid(&cx, params.udid.as_deref()).await {
        Ok(udid) => cx.debug.get()?.devices().describe(&udid).await.ok().flatten().and_then(|f| f.build_version),
        Err(_) => None,
    };
    Ok(cache.status(build.as_deref()))
}

async fn symbols_plan(
    cx: DaemonCtx,
    _rq: RequestCtx,
    params: p::DebugSymbolsPlanParams,
) -> Result<p::DebugSymbolsPlanResult> {
    let udid = resolve_udid(&cx, params.udid.as_deref()).await?;
    let files = cx.debug.get()?.devices().list_symbol_files(&udid).await?;
    SymbolCache::new(&cx.dirs.cache).plan(&files)
}

async fn symbols_fetch(
    cx: DaemonCtx,
    _rq: RequestCtx,
    params: p::DebugSymbolsFetchParams,
) -> Result<p::DebugSymbolsFetchResult> {
    if russet_debug::symbols::fetch_tier_name(params.tier).is_none() {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, "fetch tier must be `core` or `full`"));
    }
    let udid = resolve_udid(&cx, params.udid.as_deref()).await?;
    let files = cx.debug.get()?.devices().list_symbol_files(&udid).await?;
    let cache = SymbolCache::new(&cx.dirs.cache);
    // Enforce the N3 consent and space gate before any download (headless callers must pass confirmBytes).
    cache.check_fetch(&files, params.confirm_bytes, false)?;
    // The download (remoteFetchSymbols) and `ipsw dyld extract` are device-gated (DEV-17, DC-09).
    Err(russet_debug::errors::error(
        codes::debug::SYMBOLS_SERVICE,
        "the symbol download and extraction step is device-gated (DEV-17)",
    )
    .with_hint("Owner approval (Q13) and tier GREEN are required; see docs/lab/queue.md DC-09."))
}

async fn symbols_prune(
    cx: DaemonCtx,
    _rq: RequestCtx,
    params: p::DebugSymbolsPruneParams,
) -> Result<p::DebugSymbolsPruneResult> {
    let freed = SymbolCache::new(&cx.dirs.cache).prune(params.max_bytes, params.build.as_deref())?;
    Ok(p::DebugSymbolsPruneResult { freed_bytes: freed })
}
