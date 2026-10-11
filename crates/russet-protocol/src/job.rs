// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `job.*`: jobs (ARCHITECTURE §6.2, §7.6). Owner: W2.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.1 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod, RpcNotification, Unspecified};

/// Params of `job.list`: `{states?, kinds?, project?, limit?} → {jobs: JobInfo[]}`.
pub type JobListParams = Unspecified;
/// Result of `job.list`.
pub type JobListResult = Unspecified;
crate::rpc_method!(
    /// `job.list` (class `read`). W2 (Wave 1).
    JobList,
    "job.list",
    JobListParams => JobListResult,
    Permission::Read
);

/// Params of `job.get`: `{jobId} → JobInfo`.
pub type JobGetParams = Unspecified;
/// Result of `job.get`.
pub type JobGetResult = Unspecified;
crate::rpc_method!(
    /// `job.get` (class `read`). W2 (Wave 1).
    JobGet,
    "job.get",
    JobGetParams => JobGetResult,
    Permission::Read
);

/// Params of `job.wait`: `{jobId, timeoutMs?} → {job, result?, error?: ErrorData}`.
pub type JobWaitParams = Unspecified;
/// Result of `job.wait`.
pub type JobWaitResult = Unspecified;
crate::rpc_method!(
    /// `job.wait` (class `read`). W2 (Wave 1).
    JobWait,
    "job.wait",
    JobWaitParams => JobWaitResult,
    Permission::Read
);

/// Params of `job.cancel`: `{jobId} → {state} (class: owner → read, else system)`.
pub type JobCancelParams = Unspecified;
/// Result of `job.cancel`.
pub type JobCancelResult = Unspecified;
crate::rpc_method!(
    /// `job.cancel` (class `system`). W2 (Wave 1).
    JobCancel,
    "job.cancel",
    JobCancelParams => JobCancelResult,
    Permission::System, permission_fn = job_cancel_permission
);

/// Per-call class of `job.cancel` [D 66]: own job → `read`, else `system`.
///
/// F0 stub: fails closed at the ceiling (`system`) until the owner reads the params.
pub fn job_cancel_permission(
    _p: &JobCancelParams,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(Permission::System)
}

/// Params of `job.subscribe`: `{jobId?, kinds?, includeEvents?} → {subscriptionId} ⇒ job.updated, job.events`.
pub type JobSubscribeParams = Unspecified;
/// Result of `job.subscribe`.
pub type JobSubscribeResult = Unspecified;
crate::rpc_method!(
    /// `job.subscribe` (class `read`). W2 (Wave 1).
    JobSubscribe,
    "job.subscribe",
    JobSubscribeParams => JobSubscribeResult,
    Permission::Read
);

/// Params of `job.log`: `{jobId, afterSeq?, tail?, kinds?} → {events: JobEvent[], nextSeq}`.
pub type JobLogParams = Unspecified;
/// Result of `job.log`.
pub type JobLogResult = Unspecified;
crate::rpc_method!(
    /// `job.log` (class `read`). W2 (Wave 1).
    JobLog,
    "job.log",
    JobLogParams => JobLogResult,
    Permission::Read
);

/// Params of the `job.updated` notification: `{subscriptionId, job: JobInfo}; to subscribers`.
pub type JobUpdatedParams = Unspecified;
crate::rpc_notification!(
    /// `job.updated` (daemon → client). W2.
    JobUpdated,
    "job.updated",
    JobUpdatedParams
);

/// Params of the `job.events` notification: `{subscriptionId, jobId, events: JobEvent[], dropped?} (batched <= 50 ms); to subscribers`.
pub type JobEventsParams = Unspecified;
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
