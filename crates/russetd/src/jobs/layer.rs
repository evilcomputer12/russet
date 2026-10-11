// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The router layer of the daemon: every long-running method runs as a job [D 3, D 65], and audited calls
//! are written to `state/audit.jsonl` with their outcome (ARCHITECTURE §6.5, §7.6).
//!
//! A long-running request:
//! - becomes a job of kind `<method>` owned by the requesting connection; its handler gets the job's cancel
//!   token and a progress reporter that feeds the job's event log and `$/progress` (the layer sends the `begin`
//!   with the `jobId`, and the `end`);
//! - by default the request stays open until the job ends; `$/cancelRequest` cancels the job (`-32800`);
//! - with `background: true` the request returns `{jobId}` at once and the job survives the requester; its
//!   result comes from `job.wait`.

use std::time::Instant;

use russet_core::progress::ProgressSink;
use russet_core::rpc::Ownership;
use russet_ipc::{BoxFuture, Layer, Next, RequestCtx, RpcError};
use russet_protocol::MethodInfo;
use russet_protocol::audit::AuditKind;
use russet_protocol::common::{ClientRef, ConnectionRef, WorkDoneProgress};
use serde_json::Value;

use super::{JobOwner, JobSpec};
use crate::DaemonCtx;
use crate::audit::audited;
use crate::perms::{DaemonOwnership, tier_of};

/// Namespaces whose long-running methods count against `daemon.max-build-jobs` (build, and Flutter builds).
const HEAVY_NAMESPACES: &[&str] = &["build"];

/// The daemon's [`Layer`] (module docs).
#[derive(Debug, Clone)]
pub struct DaemonLayer {
    pub ownership: DaemonOwnership,
}

impl Layer<DaemonCtx> for DaemonLayer {
    fn call(
        &self,
        cx: DaemonCtx,
        rq: RequestCtx,
        info: MethodInfo,
        params: Value,
        next: Next<DaemonCtx>,
    ) -> BoxFuture<Result<Value, RpcError>> {
        let ownership = self.ownership.clone();
        Box::pin(async move {
            let started = Instant::now();
            let conn = rq.peer.connection().clone();
            let class = (info.permission_of)(&params, &conn, &ownership as &dyn Ownership).ok();
            let audit_params = class.filter(|c| audited(conn.kind, *c)).map(|c| (c, params.clone()));
            let audit = cx.audit.clone();
            let result = if info.long_running {
                run_as_job(cx, rq, info, params, next).await
            } else {
                next.run(cx, rq, params).await
            };
            if let Some((class, params)) = audit_params {
                let outcome = match &result {
                    Ok(_) => "ok".to_owned(),
                    Err(e) => e.data.as_ref().map_or_else(|| e.code.to_string(), |d| d.russet_code.clone()),
                };
                let approval = params
                    .get("approval")
                    .and_then(Value::as_str)
                    .and_then(|id| russet_core::ids::ApprovalId::parse(id).ok());
                let tier = (conn.kind == russet_core::rpc::ClientKind::Mcp).then(|| tier_of(class)).flatten();
                audit.record(&crate::audit::entry(
                    AuditKind::Rpc,
                    ClientRef { kind: conn.kind, name: conn.name.clone() },
                    info.name,
                    tier,
                    &params,
                    approval,
                    outcome,
                    russet_core::time::millis(started.elapsed()),
                ));
            }
            result
        })
    }
}

async fn run_as_job(
    cx: DaemonCtx,
    rq: RequestCtx,
    info: MethodInfo,
    params: Value,
    next: Next<DaemonCtx>,
) -> Result<Value, RpcError> {
    let background = params.get("background").and_then(Value::as_bool) == Some(true);
    let conn = rq.peer.connection().clone();
    let mut spec = JobSpec::new(info.name, info.name);
    spec.owner = Some(JobOwner {
        conn: ConnectionRef { kind: conn.kind, name: conn.name.clone(), connection_id: conn.id.clone() },
        closed: rq.peer.closed_token(),
    });
    spec.background = background;
    spec.heavy = HEAVY_NAMESPACES.contains(&info.name.split('.').next().unwrap_or_default());
    spec.project = params.get("project").and_then(Value::as_str).map(std::path::PathBuf::from);
    spec.device = params.get("udid").and_then(Value::as_str).map(str::to_owned);
    let jobs = cx.jobs.clone();
    let RequestCtx { peer, cancel, progress, request_id } = rq;
    let title = info.name;
    let handle = jobs
        .submit_raw(
            spec,
            Box::new(move |job| {
                Box::pin(async move {
                    let reporter = progress.with_tap(job.events.clone()).with_job(job.id.clone());
                    let mut begin = WorkDoneProgress::begin(title, true);
                    begin.job_id = Some(job.id.clone());
                    reporter.send(begin);
                    let inner =
                        RequestCtx { peer, cancel: job.cancel.clone(), progress: reporter.clone().begun(), request_id };
                    let result = next.run(cx, inner, params).await;
                    reporter.end(None);
                    result
                })
            }),
        )
        .map_err(RpcError::from)?;
    if background {
        return Ok(serde_json::json!({ "jobId": handle.id }));
    }
    let id = handle.id.clone();
    tokio::select! {
        r = handle.outcome() => r,
        () = cancel.cancelled() => {
            let _ = jobs.cancel(&id);
            Err(RpcError::cancelled())
        }
    }
}
