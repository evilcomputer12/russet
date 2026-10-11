// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `app` handlers (§8.2). Owner: W4 (Wave 1), W10 (app.icon).
//!
//! `app.launch` creates an `app-launch` lease (ARCHITECTURE §6.3): the launched process (and its CoreDevice launch
//! channel) stays with a task that sends `app.output` lines and `app.exited` to the caller, until the app exits or
//! the caller is gone (+ `lease-grace`). Ending the lease does not kill the app.

use russet_core::Result;
use russet_core::error::codes;
use russet_device::rpc;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::app as p;

use super::device::{caller, manager, owner_gone};
use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::AppList, _, _>(app_list);
    r.request::<p::AppInstall, _, _>(app_install);
    r.request::<p::AppUninstall, _, _>(app_uninstall);
    r.request::<p::AppLaunch, _, _>(app_launch);
    r.request::<p::AppTerminate, _, _>(app_terminate);
    r.request::<p::AppIcon, _, _>(not_implemented::<p::AppIcon>);
}

async fn app_list(cx: DaemonCtx, _rq: RequestCtx, p: p::AppListParams) -> Result<p::AppListResult> {
    rpc::app_list(manager(&cx).await?.as_ref(), p).await
}

async fn app_install(cx: DaemonCtx, rq: RequestCtx, p: p::AppInstallParams) -> Result<p::AppInstallResult> {
    rpc::app_install(manager(&cx).await?.as_ref(), &caller(&rq), p, &russet_device::NoProgress).await
}

async fn app_uninstall(cx: DaemonCtx, rq: RequestCtx, p: p::AppUninstallParams) -> Result<p::AppUninstallResult> {
    if p.bundle_id.trim().is_empty() {
        return Err(russet_core::Error::new(codes::core::INVALID_ARGUMENT, "app.uninstall needs bundleId"));
    }
    rpc::app_uninstall(manager(&cx).await?.as_ref(), &caller(&rq), p).await
}

async fn app_launch(cx: DaemonCtx, rq: RequestCtx, p: p::AppLaunchParams) -> Result<p::AppLaunchResult> {
    let m = manager(&cx).await?;
    let (_handle, launched) = rpc::app_launch(&m, &caller(&rq), p).await?;
    let lease_id = russet_device::ids::new_lease_id();
    let result =
        p::AppLaunchResult { pid: launched.pid, executable: launched.executable.clone(), lease_id: lease_id.clone() };
    let (peer, grace) = (rq.peer.clone(), m.config().lease_grace);
    tokio::spawn(async move {
        let launched = launched;
        let mut exit = launched.exit.clone();
        let mut output = launched.output.as_ref().map(|o| o.subscribe());
        let gone = owner_gone(&peer, grace);
        tokio::pin!(gone);
        // Backlog first (lines written between the launch and this task).
        if let Some((backlog, _)) = output.as_mut() {
            for e in std::mem::take(backlog) {
                let n = p::AppOutputParams {
                    lease_id: lease_id.clone(),
                    stream: p::AppOutputStream::Stdout,
                    data: e.message,
                };
                if peer.notify::<p::AppOutput>(n).is_err() {
                    return;
                }
            }
        }
        loop {
            let line = async {
                match output.as_mut() {
                    Some((_, rx)) => rx.recv().await.ok(),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = &mut gone => return,
                e = exit.wait() => {
                    let n = p::AppExitedParams {
                        lease_id: lease_id.clone(),
                        pid: launched.pid,
                        code: e.code,
                        signal: e.signal,
                        crash_id: None,
                    };
                    let _ = peer.notify::<p::AppExited>(n);
                    return;
                }
                l = line => match l {
                    Some(entry) => {
                        let n = p::AppOutputParams {
                            lease_id: lease_id.clone(),
                            stream: p::AppOutputStream::Stdout,
                            data: entry.message,
                        };
                        if peer.notify::<p::AppOutput>(n).is_err() {
                            return;
                        }
                    }
                    // The console ended or lagged: keep waiting for the exit only.
                    None => output = None,
                },
            }
        }
    });
    Ok(result)
}

async fn app_terminate(cx: DaemonCtx, rq: RequestCtx, p: p::AppTerminateParams) -> Result<p::AppTerminateResult> {
    rpc::app_terminate(manager(&cx).await?.as_ref(), &caller(&rq), p).await
}
