// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `job.*`: jobs (ARCHITECTURE §6.2, §7.6, §8.1, §9.2). Owner: W2.
//!
//! Every long-running method is also a job [D 3]: by default its request stays open until the result; with
//! `background: true` it returns [`BackgroundJob`] (`{jobId}`) at once and the job survives the requester
//! [D 65]. `job.wait` returns the result later; `job.subscribe` streams `job.updated` and `job.events`.

use russet_core::error::ErrorData;
use russet_core::ids::{JobId, SubscriptionId};
use russet_core::progress::JobEventKind;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::ConnectionRef;
use crate::{ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod, RpcNotification};

/// Job states (ARCHITECTURE §6.2): `queued → running → succeeded | failed | cancelled`, plus `interrupted`
/// after a daemon crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl JobState {
    /// Whether the job has finished (any state but `queued` and `running`).
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

/// The latest progress of a job.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub percentage: Option<u8>,
}

/// One job (`job.list`, `job.get`, `job.updated`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub job_id: JobId,
    /// `build`, `install`, `toolchain.install`, … (ARCHITECTURE §6.2).
    pub kind: String,
    pub title: String,
    pub state: JobState,
    /// The requesting connection; jobs the daemon starts itself have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub owner: Option<ConnectionRef>,
    /// Survives its owner's disconnect [D 65].
    pub background: bool,
    /// The absolute project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    /// The device's UDID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub device: Option<String>,
    /// RFC 3339 UTC.
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub progress: Option<JobProgress>,
    /// While `queued`: what it waits for (a lock or a free slot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub waiting_for: Option<String>,
    /// When `failed` (or `cancelled`, `interrupted`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<ErrorData>,
}

/// One structured job event (ARCHITECTURE §9.2): `{seq, ts, kind, …}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobEvent {
    /// Starts at 1 and grows by one per event of the job.
    pub seq: u64,
    /// RFC 3339 UTC.
    pub ts: String,
    #[serde(flatten)]
    pub event: JobEventKind,
}

/// What a long-running method returns with `background: true` [D 65].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundJob {
    pub job_id: JobId,
}

/// Params of `job.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobListParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<JobState>,
    /// Job kinds (`build`, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    /// Newest first; default 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `job.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobListResult {
    pub jobs: Vec<JobInfo>,
}

crate::rpc_method!(
    /// `job.list` (class `read`). W2 (Wave 1).
    JobList,
    "job.list",
    JobListParams => JobListResult,
    Permission::Read
);

/// Params of `job.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobGetParams {
    pub job_id: JobId,
}

/// Result of `job.get`.
pub type JobGetResult = JobInfo;

crate::rpc_method!(
    /// `job.get` (class `read`). W2 (Wave 1).
    JobGet,
    "job.get",
    JobGetParams => JobGetResult,
    Permission::Read
);

/// Params of `job.wait`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobWaitParams {
    pub job_id: JobId,
    /// Return the current state after this long even if the job still runs; default: wait until it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub timeout_ms: Option<u64>,
}

/// Result of `job.wait`: the job, plus its result or error once it ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobWaitResult {
    pub job: JobInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<ErrorData>,
}

crate::rpc_method!(
    /// `job.wait` (class `read`). W2 (Wave 1).
    JobWait,
    "job.wait",
    JobWaitParams => JobWaitResult,
    Permission::Read
);

/// Params of `job.cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobCancelParams {
    pub job_id: JobId,
}

/// Result of `job.cancel`: the state after the request (`cancelled`, or the terminal state it already had).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobCancelResult {
    pub state: JobState,
}

crate::rpc_method!(
    /// `job.cancel` (class: the owner → `read`, anyone else → `system`; resolved per call, ceiling `system`).
    /// W2 (Wave 1).
    JobCancel,
    "job.cancel",
    JobCancelParams => JobCancelResult,
    Permission::System, permission_fn = job_cancel_permission
);

/// Per-call class of `job.cancel` [D 66]: own job → `read`, else `system`.
pub fn job_cancel_permission(p: &JobCancelParams, c: &ConnInfo, o: &dyn Ownership) -> russet_core::Result<Permission> {
    Ok(if o.owns_job(&c.id, &p.job_id) { Permission::Read } else { Permission::System })
}

/// Params of `job.subscribe`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobSubscribeParams {
    /// One job; omitted: every job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub job_id: Option<JobId>,
    /// Only jobs of these kinds (`build`, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    /// Also stream `job.events`.
    #[serde(default)]
    pub include_events: bool,
}

/// Result of `job.subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobSubscribeResult {
    pub subscription_id: SubscriptionId,
}

crate::rpc_method!(
    /// `job.subscribe` (class `read`). W2 (Wave 1).
    JobSubscribe,
    "job.subscribe",
    JobSubscribeParams => JobSubscribeResult,
    Permission::Read
);

/// Params of `job.log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobLogParams {
    pub job_id: JobId,
    /// Only events with a larger `seq`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub after_seq: Option<u64>,
    /// Only the last `tail` matching events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tail: Option<u32>,
    /// Only these event kinds (`log`, `diagnostic`, …; ARCHITECTURE §9.2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
}

/// Result of `job.log`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobLogResult {
    pub events: Vec<JobEvent>,
    /// Pass as `afterSeq` to continue.
    pub next_seq: u64,
}

crate::rpc_method!(
    /// `job.log` (class `read`). W2 (Wave 1).
    JobLog,
    "job.log",
    JobLogParams => JobLogResult,
    Permission::Read
);

/// Params of the `job.updated` notification: every state change, including the result summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobUpdatedParams {
    pub subscription_id: SubscriptionId,
    pub job: JobInfo,
}
crate::rpc_notification!(
    /// `job.updated` (daemon → client). W2.
    JobUpdated,
    "job.updated",
    JobUpdatedParams
);

/// Params of the `job.events` notification (batched for at most 50 ms).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobEventsParams {
    pub subscription_id: SubscriptionId,
    pub job_id: JobId,
    pub events: Vec<JobEvent>,
    /// Events lost to a slow consumer since the previous batch (ARCHITECTURE §6.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dropped: Option<u64>,
}
crate::rpc_notification!(
    /// `job.events` (daemon → client). W2.
    JobEvents,
    "job.events",
    JobEventsParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] =
    &[JobList::INFO, JobGet::INFO, JobWait::INFO, JobCancel::INFO, JobSubscribe::INFO, JobLog::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[JobUpdated::INFO, JobEvents::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];

#[cfg(test)]
mod tests {
    use russet_core::progress::LogStream;
    use serde_json::json;

    use super::*;

    #[test]
    fn job_events_flatten_their_kind() {
        let e = JobEvent {
            seq: 3,
            ts: "2026-10-11T09:00:00.000Z".into(),
            event: JobEventKind::Log { stream: LogStream::Stdout, line: "hi".into() },
        };
        let v = serde_json::to_value(&e).expect("serializes");
        assert_eq!(
            v,
            json!({"seq": 3, "ts": "2026-10-11T09:00:00.000Z", "kind": "log", "stream": "stdout", "line": "hi"})
        );
        assert_eq!(serde_json::from_value::<JobEvent>(v).expect("parses"), e);
    }

    #[test]
    fn terminal_states() {
        assert!(!JobState::Queued.is_terminal() && !JobState::Running.is_terminal());
        for s in [JobState::Succeeded, JobState::Failed, JobState::Cancelled, JobState::Interrupted] {
            assert!(s.is_terminal());
        }
        assert_eq!(serde_json::to_value(JobState::Interrupted).expect("serializes"), json!("interrupted"));
    }
}
