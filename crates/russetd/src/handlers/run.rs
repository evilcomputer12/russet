// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `run` handlers (ARCHITECTURE §8.5). Owner: W7 (Wave 1: `run.start`, `run.list`, `run.subscribe`,
//! `run.output.query`, `run.stop`), W11 (`run.relaunch`, `run.hotReload`, Wave 2), W18 (Flutter, Wave 3).
//!
//! The work is in [`crate::pipeline`]; these functions only bind it to the request: the requester's peer for
//! notifications and for the owned-session lease (owner gone + 2 s, ARCHITECTURE §6.3).

use russet_core::Result;
use russet_core::ids::SubscriptionId;
use russet_debug::SessionHandle;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::run as p;

use crate::DaemonCtx;
use crate::pipeline::live::pipeline_for;
use crate::pipeline::run::TracingSink;
use crate::pipeline::subscribe::{RunNotification, forward_session};

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::RunStart, _, _>(start);
    r.request::<p::RunList, _, _>(list);
    r.request::<p::RunSubscribe, _, _>(subscribe);
    r.request::<p::RunOutputQuery, _, _>(output_query);
    r.request::<p::RunStop, _, _>(stop);
    r.request::<p::RunRelaunch, _, _>(relaunch);
    r.request::<p::RunHotReload, _, _>(hot_reload);
}

async fn relaunch(cx: DaemonCtx, _rq: RequestCtx, params: p::RunRelaunchParams) -> Result<p::RunRelaunchResult> {
    let (pid, ms) = cx.debug.get()?.relaunch(params.session_id.as_str()).await?;
    Ok(p::RunRelaunchResult { pid, ms })
}

async fn hot_reload(cx: DaemonCtx, _rq: RequestCtx, params: p::RunHotReloadParams) -> Result<p::RunHotReloadResult> {
    let h = cx
        .debug
        .get()?
        .session(&params.session_id)
        .ok_or_else(|| russet_debug::errors::session_not_found(params.session_id.as_str()))?;
    // Swift sessions support only Quick Relaunch (a restart); Flutter hot reload lands with W18.
    let full = matches!(params.kind, Some(p::HotReloadKind::Restart));
    let r = h.hot_reload(full).await?;
    Ok(p::RunHotReloadResult {
        supported: r.supported,
        kind: if full { p::HotReloadKind::Restart } else { p::HotReloadKind::Reload },
        ms: r.ms,
        message: r.message,
    })
}

/// An owned session ends 2 s after its requester's connection closes (ARCHITECTURE §6.3).
pub fn bind_owner(cx: &DaemonCtx, rq: &RequestCtx, h: &SessionHandle) {
    let peer = rq.peer.clone();
    let grace =
        cx.debug.try_get().map_or_else(|| russet_debug::DebugConfig::default().lease_grace, |d| d.config().lease_grace);
    h.end_with_owner(async move { peer.closed().await }, grace);
}

async fn start(cx: DaemonCtx, rq: RequestCtx, params: p::RunStartParams) -> Result<p::RunStartResult> {
    let pipeline = pipeline_for(&cx)?;
    let (result, session) = pipeline.start(params, &TracingSink, &rq.cancel).await?;
    if let Some(h) = &session {
        bind_owner(&cx, &rq, h);
    }
    Ok(result)
}

async fn list(cx: DaemonCtx, _rq: RequestCtx, _params: p::RunListParams) -> Result<p::RunListResult> {
    Ok(pipeline_for(&cx)?.list())
}

async fn output_query(
    cx: DaemonCtx,
    _rq: RequestCtx,
    params: p::RunOutputQueryParams,
) -> Result<p::RunOutputQueryResult> {
    pipeline_for(&cx)?.output_query(&params)
}

async fn stop(cx: DaemonCtx, _rq: RequestCtx, params: p::RunStopParams) -> Result<p::RunStopResult> {
    pipeline_for(&cx)?.stop(&params).await
}

async fn subscribe(cx: DaemonCtx, rq: RequestCtx, params: p::RunSubscribeParams) -> Result<p::RunSubscribeResult> {
    let sub: SubscriptionId = russet_debug::session::prefixed_ulid(SubscriptionId::PREFIX);
    let peer = rq.peer.clone();
    let send = move |n: RunNotification| -> bool {
        let sent = match n {
            RunNotification::State(s) => peer.notify::<p::RunState>(s),
            RunNotification::Output(o) => peer.notify::<p::RunOutput>(o),
            RunNotification::Exited(e) => peer.notify::<p::RunExited>(e),
        };
        sent.is_ok()
    };
    let closed = {
        let peer = rq.peer.clone();
        move || {
            let peer = peer.clone();
            async move { peer.closed().await }
        }
    };
    match &params.session_id {
        Some(id) => {
            let h = cx.debug.get()?.session(id).ok_or_else(|| russet_debug::errors::session_not_found(id.as_str()))?;
            tokio::spawn(forward_session(h, sub.clone(), send, closed()));
        }
        None => {
            // Every current session, then every new one, until the subscriber goes away.
            let debug = cx.debug.get()?;
            let mut created = debug.watch_new();
            for h in debug.handles().into_iter().filter(|h| !h.is_ended()) {
                tokio::spawn(forward_session(h, sub.clone(), send.clone(), closed()));
            }
            let sub2 = sub.clone();
            let gone = closed();
            tokio::spawn(async move {
                tokio::pin!(gone);
                loop {
                    tokio::select! {
                        () = &mut gone => break,
                        h = created.recv() => match h {
                            Ok(h) => { tokio::spawn(forward_session(h, sub2.clone(), send.clone(), closed())); }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        },
                    }
                }
            });
        }
    }
    Ok(p::RunSubscribeResult { subscription_id: sub })
}
