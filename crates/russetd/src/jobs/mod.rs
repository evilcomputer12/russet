// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Jobs: queue, locks ([`LockKey`]), limits, the cancellation tree, persistence, retention (ARCHITECTURE §6.2,
//! §7.6; core §6.4).
//!
//! - **States:** `queued → running → succeeded | failed | cancelled`, plus `interrupted` for jobs a crashed
//!   daemon left running (found at the next start).
//! - **Scheduling:** at most `max-parallel-jobs` (4) run, at most `max-build-jobs` (1) heavy ones, and a job
//!   waits (`queued`, with `waitingFor`) while another job holds one of its locks.
//! - **Cancellation:** `job.cancel`, `$/cancelRequest` of the request that waits for it, the owner's
//!   disconnect (unless `background` [D 65]) or daemon shutdown cancel the job's token; the job then has
//!   [`JOB_GRACE`] (10 s) to return before its task is aborted, which kills its process trees.
//! - **Persistence:** `state/jobs/index.jsonl` (one line per state change), `state/jobs/<id>/events.ndjson`
//!   (every [`JobEvent`]) and `state/jobs/<id>/result.json` (`{job, result?, error?}`); a job's events are also
//!   mirrored to `JobSpec::mirror_log`, or `<mirror_dir>/<job-id>.jsonl` (build jobs:
//!   `<project>/.russet/logs/<job-id>.jsonl`). Only the daemon that holds `runtime/russetd.lock` persists jobs
//!   and recovers them; an in-process daemon of a home whose daemon runs keeps its jobs in memory.
//! - **Retention:** the newest 200 jobs, none older than 30 days; compacted at start.
//! - **Declarations:** what a long-running method's job locks, and whether it is heavy, is declared by the
//!   method's area ([`declarations`]); code outside jobs takes the same locks with [`JobManager::lock`].

pub mod declarations;
pub mod layer;

pub use declarations::{JobDeclarations, JobOptions, declare};

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

use russet_core::error::{Error, Result, codes};
use russet_core::ids::JobId;
use russet_core::progress::{EventSink, JobEventKind, LogStream, ProgressSink};
use russet_ipc::{BoxFuture, RpcError};
use russet_protocol::common::ConnectionRef;
use russet_protocol::daemon::JobCounts;
use russet_protocol::job::{
    JobEvent, JobInfo, JobListParams, JobLogParams, JobLogResult, JobProgress, JobState, JobWaitResult,
};
use serde_json::Value;
use tokio::sync::{Notify, broadcast, watch};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

use crate::state::{JsonLines, read_lines};

/// How long a cancelled job may take to return before its task is aborted.
pub const JOB_GRACE: Duration = Duration::from_secs(10);
/// Jobs kept on disk.
pub const KEEP_JOBS: usize = 200;
/// Days a finished job is kept.
pub const KEEP_DAYS: u64 = 30;
/// Events of one job kept in memory for `job.log` and subscribers (all of them are on disk).
pub const EVENT_RING: usize = 10_000;

/// What a job locks exclusively (ARCHITECTURE §6.2). Code that takes more than one lock over time takes them
/// in this order (project, device, SDK, named), so two holders never wait for each other.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LockKey {
    /// One build per project root.
    Project(PathBuf),
    /// Install, uninstall, DDI mount, pairing and device reservation.
    Device(String),
    /// `sdk.import`, `sdk.remove`.
    Sdk,
    /// Anything else, by name.
    Named(String),
}

impl LockKey {
    fn describe(&self) -> String {
        match self {
            Self::Project(p) => format!("the project lock of {}", p.display()),
            Self::Device(u) => format!("the device lock of {}", u.chars().take(8).collect::<String>()),
            Self::Sdk => "the SDK lock".to_owned(),
            Self::Named(n) => format!("the {n} lock"),
        }
    }
}

/// Concurrency limits (`daemon.max-parallel-jobs`, `daemon.max-build-jobs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobLimits {
    pub max_parallel: usize,
    pub max_heavy: usize,
}

impl Default for JobLimits {
    fn default() -> Self {
        Self { max_parallel: 4, max_heavy: 1 }
    }
}

/// The connection a job belongs to.
#[derive(Debug, Clone)]
pub struct JobOwner {
    pub conn: ConnectionRef,
    /// Fires when the owner disconnects.
    pub closed: CancellationToken,
}

/// What to run.
#[derive(Debug, Clone)]
pub struct JobSpec {
    /// `build`, `install`, `toolchain.install`, … (for long-running methods: the method name).
    pub kind: String,
    pub title: String,
    pub project: Option<PathBuf>,
    pub device: Option<String>,
    pub locks: Vec<LockKey>,
    pub owner: Option<JobOwner>,
    /// Survives the owner's disconnect [D 65].
    pub background: bool,
    /// Counts against `max-build-jobs`.
    pub heavy: bool,
    /// Also append every event to this JSON-lines file.
    pub mirror_log: Option<PathBuf>,
    /// Also append every event to `<mirror_dir>/<job-id>.jsonl` (when `mirror_log` is not set).
    pub mirror_dir: Option<PathBuf>,
}

impl JobSpec {
    /// A job of `kind` titled `title`.
    pub fn new(kind: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            title: title.into(),
            project: None,
            device: None,
            locks: Vec::new(),
            owner: None,
            background: false,
            heavy: false,
            mirror_log: None,
            mirror_dir: None,
        }
    }
}

/// What a running job gets.
#[derive(Clone)]
pub struct JobCtx {
    pub id: JobId,
    /// Fired by `job.cancel`, the owner's disconnect (unless `background`) or shutdown.
    pub cancel: CancellationToken,
    /// The job's event sink: `events.ndjson`, `job.events` subscribers and `job.get`'s progress.
    pub events: Arc<JobEvents>,
}

impl std::fmt::Debug for JobCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobCtx").field("id", &self.id).finish_non_exhaustive()
    }
}

/// A change the daemon announces to `job.subscribe` subscribers.
#[derive(Debug, Clone)]
pub enum JobNotice {
    Updated(Box<JobInfo>),
    Event { job_id: JobId, event: JobEvent },
}

type Outcome = std::result::Result<Value, RpcError>;
type JobFn = Box<dyn FnOnce(JobCtx) -> BoxFuture<Outcome> + Send>;

struct EventLog {
    next_seq: u64,
    ring: VecDeque<JobEvent>,
}

struct JobEntry {
    id: JobId,
    info: Mutex<JobInfo>,
    cancel: CancellationToken,
    owner_conn: Option<String>,
    locks: Vec<LockKey>,
    heavy: bool,
    events: Mutex<EventLog>,
    outcome: Mutex<Option<Outcome>>,
    done: watch::Sender<bool>,
    abort: Mutex<Option<AbortHandle>>,
    dir: Option<PathBuf>,
    mirror: Option<PathBuf>,
    /// Loaded from the index of an earlier daemon run: no task, result on disk.
    historic: bool,
}

#[derive(Default)]
struct Slots {
    running: usize,
    heavy: usize,
    held: BTreeSet<LockKey>,
}

struct Inner {
    root: Option<PathBuf>,
    index: Option<JsonLines>,
    jobs: Mutex<BTreeMap<JobId, Arc<JobEntry>>>,
    limits: Mutex<JobLimits>,
    slots: Mutex<Slots>,
    wake: Notify,
    updates: broadcast::Sender<JobNotice>,
    shutdown: CancellationToken,
    grace: Mutex<Duration>,
}

/// A cheap-to-clone handle (`DaemonCtx.jobs`).
#[derive(Clone)]
pub struct JobManager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for JobManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobManager").field("root", &self.inner.root).field("jobs", &self.lock_jobs().len()).finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn not_found(id: &JobId) -> Error {
    Error::new(codes::core::NOT_FOUND, format!("job {id} does not exist"))
        .with_hint("List the jobs with `russet daemon status` (jobs older than 30 days are gone).")
}

impl JobManager {
    /// A manager persisting under `root` (`state/jobs`; `None` keeps everything in memory). It loads the jobs of
    /// earlier runs, marks the ones a crash left `queued` or `running` as `interrupted`, and compacts to the
    /// retention limits. Jobs are cancelled when `shutdown` fires.
    pub fn new(root: Option<PathBuf>, limits: JobLimits, shutdown: CancellationToken) -> Self {
        let (updates, _) = broadcast::channel(1024);
        let index = root.as_ref().map(|r| JsonLines::new(r.join("index.jsonl")));
        let mgr = Self {
            inner: Arc::new(Inner {
                root,
                index,
                jobs: Mutex::new(BTreeMap::new()),
                limits: Mutex::new(limits),
                slots: Mutex::new(Slots::default()),
                wake: Notify::new(),
                updates,
                shutdown,
                grace: Mutex::new(JOB_GRACE),
            }),
        };
        if let Err(e) = mgr.recover() {
            tracing::warn!("cannot read the job index: {}", e.message());
        }
        mgr
    }

    fn lock_jobs(&self) -> std::sync::MutexGuard<'_, BTreeMap<JobId, Arc<JobEntry>>> {
        lock(&self.inner.jobs)
    }

    /// Whether jobs are persisted under `state/jobs`.
    pub fn persistent(&self) -> bool {
        self.inner.root.is_some()
    }

    /// The concurrency limits.
    pub fn limits(&self) -> JobLimits {
        *lock(&self.inner.limits)
    }

    /// Changes the concurrency limits (`daemon.max-parallel-jobs`, `daemon.max-build-jobs`); queued jobs start if
    /// the new limits allow.
    pub fn set_limits(&self, limits: JobLimits) {
        *lock(&self.inner.limits) =
            JobLimits { max_parallel: limits.max_parallel.max(1), max_heavy: limits.max_heavy.max(1) };
        self.inner.wake.notify_waiters();
    }

    /// Takes `keys` for code that does not run as a job (`app.uninstall`, `sdk.remove`; ARCHITECTURE §6.2): waits
    /// while a job or another holder has one of them, and holds them until the guard drops. `cancel` gives up
    /// with `core.cancelled`. Inside a job, take further locks in [`LockKey`] order.
    pub async fn lock(&self, keys: Vec<LockKey>, cancel: &CancellationToken) -> Result<HeldLocks> {
        loop {
            let woken = self.inner.wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            {
                let mut s = lock(&self.inner.slots);
                if !keys.iter().any(|k| s.held.contains(k)) {
                    s.held.extend(keys.iter().cloned());
                    return Ok(HeldLocks { keys, inner: Arc::downgrade(&self.inner) });
                }
            }
            tokio::select! {
                () = &mut woken => {}
                () = cancel.cancelled() => {
                    return Err(Error::new(codes::core::CANCELLED, "cancelled while waiting for a lock"));
                }
            }
        }
    }

    /// Loads the index of earlier runs; `queued`/`running` jobs become `interrupted`. Returns how many were
    /// interrupted.
    fn recover(&self) -> Result<usize> {
        let (Some(root), Some(index)) = (&self.inner.root, &self.inner.index) else { return Ok(0) };
        let lines: Vec<JobInfo> = index.read_all()?;
        let mut latest: BTreeMap<JobId, JobInfo> = BTreeMap::new();
        for info in lines {
            latest.insert(info.job_id.clone(), info);
        }
        let now = SystemTime::now();
        let mut interrupted = 0;
        for info in latest.values_mut() {
            if !info.state.is_terminal() {
                info.state = JobState::Interrupted;
                info.ended_at = Some(russet_core::time::rfc3339(now));
                info.error = Some(
                    Error::new(codes::core::INTERNAL, "the daemon stopped while this job ran")
                        .with_hint("Run it again.")
                        .to_data(),
                );
                interrupted += 1;
                let result = JobWaitResult { job: info.clone(), result: None, error: info.error.clone() };
                let _ = write_result(&root.join(info.job_id.as_str()), &result);
            }
        }
        // Retention: newest KEEP_JOBS, none older than KEEP_DAYS.
        let cutoff = now - Duration::from_secs(KEEP_DAYS * 86_400);
        let mut all: Vec<JobInfo> = latest.into_values().collect();
        // Newest first: ULIDs sort by creation time.
        all.sort_by(|a, b| b.job_id.cmp(&a.job_id));
        let mut keep = Vec::new();
        for (i, info) in all.into_iter().enumerate() {
            let recent = info
                .job_id
                .created_millis()
                .is_none_or(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms) >= cutoff);
            if i < KEEP_JOBS && recent {
                keep.push(info);
            } else {
                let _ = std::fs::remove_dir_all(root.join(info.job_id.as_str()));
            }
        }
        // Rewrite the index with one line per kept job, oldest first.
        let mut text = Vec::new();
        for info in keep.iter().rev() {
            if let Ok(line) = serde_json::to_vec(info) {
                text.extend_from_slice(&line);
                text.push(b'\n');
            }
        }
        russet_core::fsx::ensure_private_dir(root)?;
        russet_core::fsx::atomic_write(index.path(), &text)?;
        let mut jobs = self.lock_jobs();
        for info in keep {
            let (done, _) = watch::channel(true);
            let entry = JobEntry {
                id: info.job_id.clone(),
                cancel: CancellationToken::new(),
                owner_conn: None,
                locks: Vec::new(),
                heavy: false,
                events: Mutex::new(EventLog { next_seq: 1, ring: VecDeque::new() }),
                outcome: Mutex::new(None),
                done,
                abort: Mutex::new(None),
                dir: Some(root.join(info.job_id.as_str())),
                mirror: None,
                historic: true,
                info: Mutex::new(info),
            };
            jobs.insert(entry.id.clone(), Arc::new(entry));
        }
        Ok(interrupted)
    }

    /// Starts a job running `f` (queued until its locks and a slot are free). The job's result is `f`'s output
    /// serialized as JSON.
    pub fn submit<T, F, Fut>(&self, spec: JobSpec, f: F) -> Result<JobHandle>
    where
        T: serde::Serialize + Send + 'static,
        F: FnOnce(JobCtx) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
    {
        self.submit_raw(
            spec,
            Box::new(move |ctx| {
                Box::pin(async move {
                    let out = f(ctx).await.map_err(RpcError::from)?;
                    serde_json::to_value(out)
                        .map_err(|e| RpcError::internal(format!("cannot serialize a job result: {e}")))
                })
            }),
        )
    }

    /// [`JobManager::submit`] for a job whose outcome is already JSON or a JSON-RPC error (the long-running
    /// method layer).
    pub fn submit_raw(&self, spec: JobSpec, f: JobFn) -> Result<JobHandle> {
        if self.inner.shutdown.is_cancelled() {
            return Err(Error::new(codes::ipc::CONNECTION_CLOSED, "the daemon is shutting down").retryable(true));
        }
        let id = JobId::generate();
        let mirror =
            spec.mirror_log.clone().or_else(|| spec.mirror_dir.as_ref().map(|d| d.join(format!("{id}.jsonl"))));
        let dir = self.inner.root.as_ref().map(|r| r.join(id.as_str()));
        if let Some(d) = &dir {
            russet_core::fsx::ensure_private_dir(d)?;
        }
        let info = JobInfo {
            job_id: id.clone(),
            kind: spec.kind.clone(),
            title: spec.title.clone(),
            state: JobState::Queued,
            owner: spec.owner.as_ref().map(|o| o.conn.clone()),
            background: spec.background,
            project: spec.project.as_ref().map(|p| p.display().to_string()),
            device: spec.device.clone(),
            created_at: russet_core::time::now_rfc3339(),
            started_at: None,
            ended_at: None,
            duration_ms: None,
            progress: None,
            waiting_for: None,
            error: None,
        };
        let (done, _) = watch::channel(false);
        let entry = Arc::new(JobEntry {
            id: id.clone(),
            info: Mutex::new(info),
            cancel: self.inner.shutdown.child_token(),
            owner_conn: spec.owner.as_ref().map(|o| o.conn.connection_id.clone()),
            locks: spec.locks.clone(),
            heavy: spec.heavy,
            events: Mutex::new(EventLog { next_seq: 1, ring: VecDeque::new() }),
            outcome: Mutex::new(None),
            done,
            abort: Mutex::new(None),
            dir,
            mirror,
            historic: false,
        });
        self.lock_jobs().insert(id.clone(), Arc::clone(&entry));
        self.publish_info(&entry);
        if let (Some(owner), false) = (&spec.owner, spec.background) {
            let (token, closed) = (entry.cancel.clone(), owner.closed.clone());
            let mut done = entry.done.subscribe();
            tokio::spawn(async move {
                tokio::select! {
                    () = closed.cancelled() => token.cancel(),
                    _ = done.wait_for(|d| *d) => {}
                }
            });
        }
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(run_job(weak, Arc::clone(&entry), f));
        Ok(JobHandle { id, done: entry.done.subscribe(), manager: self.clone() })
    }

    fn publish_info(&self, entry: &JobEntry) {
        publish_info(&self.inner, entry);
    }

    /// Jobs matching `p`, newest first (default limit 50).
    pub fn list(&self, p: &JobListParams) -> Vec<JobInfo> {
        let limit = p.limit.map_or(50, |l| l as usize);
        let jobs = self.lock_jobs();
        jobs.values()
            .rev()
            .map(|e| lock(&e.info).clone())
            .filter(|i| p.states.is_empty() || p.states.contains(&i.state))
            .filter(|i| p.kinds.is_empty() || p.kinds.contains(&i.kind))
            .filter(|i| p.project.as_ref().is_none_or(|proj| i.project.as_ref() == Some(proj)))
            .take(limit)
            .collect()
    }

    /// One job.
    pub fn get(&self, id: &JobId) -> Result<JobInfo> {
        self.lock_jobs().get(id).map(|e| lock(&e.info).clone()).ok_or_else(|| not_found(id))
    }

    fn entry(&self, id: &JobId) -> Result<Arc<JobEntry>> {
        self.lock_jobs().get(id).cloned().ok_or_else(|| not_found(id))
    }

    /// The job once it ended (or its current state after `timeout`), with its result or error.
    pub async fn wait(&self, id: &JobId, timeout: Option<Duration>) -> Result<JobWaitResult> {
        let entry = self.entry(id)?;
        let mut done = entry.done.subscribe();
        let finished = async { done.wait_for(|d| *d).await.is_ok() };
        match timeout {
            Some(t) => {
                let _ = tokio::time::timeout(t, finished).await;
            }
            None => {
                finished.await;
            }
        }
        Ok(self.wait_result(&entry))
    }

    fn wait_result(&self, entry: &JobEntry) -> JobWaitResult {
        let info = lock(&entry.info).clone();
        if entry.historic {
            if let Some(dir) = &entry.dir
                && let Ok(text) = std::fs::read(dir.join("result.json"))
                && let Ok(r) = serde_json::from_slice::<JobWaitResult>(&text)
            {
                return JobWaitResult { job: info, ..r };
            }
            return JobWaitResult { error: info.error.clone(), job: info, result: None };
        }
        match lock(&entry.outcome).as_ref() {
            Some(Ok(v)) => JobWaitResult { job: info, result: Some(v.clone()), error: None },
            Some(Err(e)) => JobWaitResult { job: info, result: None, error: e.data.as_deref().cloned() },
            None => JobWaitResult { job: info, result: None, error: None },
        }
    }

    /// The outcome of a finished job, with its JSON-RPC error intact (the long-running method layer).
    pub(crate) fn outcome(&self, id: &JobId) -> Option<Outcome> {
        let entry = self.entry(id).ok()?;
        lock(&entry.outcome).clone()
    }

    /// Cancels a job; returns its state after the request (`cancelled` once it stops, or the terminal state it
    /// already had).
    pub fn cancel(&self, id: &JobId) -> Result<JobState> {
        let entry = self.entry(id)?;
        let state = lock(&entry.info).state;
        if state.is_terminal() {
            return Ok(state);
        }
        entry.cancel.cancel();
        Ok(JobState::Cancelled)
    }

    /// Job changes and events for `job.subscribe`.
    pub fn subscribe(&self) -> broadcast::Receiver<JobNotice> {
        self.inner.updates.subscribe()
    }

    /// Events of a job (`job.log`).
    pub fn log(&self, p: &JobLogParams) -> Result<JobLogResult> {
        let entry = self.entry(&p.job_id)?;
        let after = p.after_seq.unwrap_or(0);
        let wanted = |e: &JobEvent| e.seq > after && (p.kinds.is_empty() || p.kinds.iter().any(|k| k == e.event.tag()));
        let mut events: Vec<JobEvent> = {
            let log = lock(&entry.events);
            let in_memory_from = log.ring.front().map_or(log.next_seq, |e| e.seq);
            if entry.historic || after + 1 < in_memory_from {
                drop(log);
                match &entry.dir {
                    Some(dir) => {
                        read_lines::<JobEvent>(&dir.join("events.ndjson"))?.into_iter().filter(wanted).collect()
                    }
                    None => Vec::new(),
                }
            } else {
                log.ring.iter().filter(|e| wanted(e)).cloned().collect()
            }
        };
        if let Some(tail) = p.tail {
            let skip = events.len().saturating_sub(tail as usize);
            events.drain(..skip);
        }
        let next_seq = events.last().map_or(after, |e| e.seq);
        Ok(JobLogResult { events, next_seq })
    }

    /// Whether connection `conn_id` owns job `id`.
    pub fn owns(&self, conn_id: &str, id: &JobId) -> bool {
        self.lock_jobs().get(id).is_some_and(|e| e.owner_conn.as_deref() == Some(conn_id))
    }

    /// Queued and running jobs.
    pub fn counts(&self) -> JobCounts {
        let jobs = self.lock_jobs();
        let mut c = JobCounts::default();
        for e in jobs.values() {
            match lock(&e.info).state {
                JobState::Queued => c.queued += 1,
                JobState::Running => c.running += 1,
                _ => {}
            }
        }
        c
    }

    /// Cancels every job, waits up to `grace` for them to return, then aborts what is left (killing process
    /// trees) (ARCHITECTURE §3.3 item 5).
    pub async fn shutdown(&self, grace: Duration) {
        *lock(&self.inner.grace) = grace;
        let live: Vec<Arc<JobEntry>> = self.lock_jobs().values().filter(|e| !*e.done.borrow()).cloned().collect();
        for e in &live {
            e.cancel.cancel();
        }
        let deadline = tokio::time::Instant::now() + grace + Duration::from_millis(200);
        for e in &live {
            let mut done = e.done.subscribe();
            if tokio::time::timeout_at(deadline, done.wait_for(|d| *d)).await.is_err()
                && let Some(a) = lock(&e.abort).as_ref()
            {
                a.abort();
            }
        }
    }
}

fn publish_info(inner: &Inner, entry: &JobEntry) {
    let info = lock(&entry.info).clone();
    if let Some(index) = &inner.index
        && let Err(e) = index.append(&info)
    {
        tracing::warn!("cannot update the job index: {}", e.message());
    }
    let _ = inner.updates.send(JobNotice::Updated(Box::new(info)));
}

fn write_result(dir: &std::path::Path, r: &JobWaitResult) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(r).map_err(|e| Error::internal(format!("encoding result.json: {e}")))?;
    russet_core::fsx::atomic_write(&dir.join("result.json"), &bytes)
}

impl Inner {
    fn try_start(&self, entry: &JobEntry) -> std::result::Result<(), String> {
        let mut s = lock(&self.slots);
        if let Some(l) = entry.locks.iter().find(|l| s.held.contains(*l)) {
            return Err(l.describe());
        }
        let limits = *lock(&self.limits);
        if s.running >= limits.max_parallel {
            return Err(format!("a free job slot ({} jobs run)", s.running));
        }
        if entry.heavy && s.heavy >= limits.max_heavy {
            return Err("the build slot (daemon.max-build-jobs)".to_owned());
        }
        s.running += 1;
        if entry.heavy {
            s.heavy += 1;
        }
        s.held.extend(entry.locks.iter().cloned());
        Ok(())
    }

    fn release(&self, entry: &JobEntry) {
        let mut s = lock(&self.slots);
        s.running = s.running.saturating_sub(1);
        if entry.heavy {
            s.heavy = s.heavy.saturating_sub(1);
        }
        for l in &entry.locks {
            s.held.remove(l);
        }
        drop(s);
        self.wake.notify_waiters();
    }
}

async fn run_job(inner: Weak<Inner>, entry: Arc<JobEntry>, f: JobFn) {
    let Some(mgr) = inner.upgrade() else { return };
    // Wait for the locks and a slot.
    loop {
        let woken = mgr.wake.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();
        match mgr.try_start(&entry) {
            Ok(()) => break,
            Err(waiting) => {
                let changed = {
                    let mut info = lock(&entry.info);
                    let changed = info.waiting_for.as_deref() != Some(waiting.as_str());
                    info.waiting_for = Some(waiting);
                    changed
                };
                if changed {
                    publish_info(&mgr, &entry);
                }
            }
        }
        tokio::select! {
            () = &mut woken => {}
            () = entry.cancel.cancelled() => {
                finish(&mgr, &entry, Err(RpcError::cancelled()), true);
                return;
            }
        }
    }
    {
        let mut info = lock(&entry.info);
        info.state = JobState::Running;
        info.waiting_for = None;
        info.started_at = Some(russet_core::time::now_rfc3339());
    }
    publish_info(&mgr, &entry);
    let events = Arc::new(JobEvents { entry: Arc::clone(&entry), inner: Arc::downgrade(&mgr) });
    let ctx = JobCtx { id: entry.id.clone(), cancel: entry.cancel.clone(), events };
    let mut task = tokio::spawn(f(ctx));
    *lock(&entry.abort) = Some(task.abort_handle());
    let joined = tokio::select! {
        r = &mut task => r,
        () = entry.cancel.cancelled() => {
            let grace = *lock(&mgr.grace);
            match tokio::time::timeout(grace, &mut task).await {
                Ok(r) => r,
                Err(_) => {
                    task.abort();
                    let _ = (&mut task).await;
                    Ok(Err(RpcError::cancelled()))
                }
            }
        }
    };
    let outcome = match joined {
        Ok(o) => o,
        Err(e) if e.is_panic() => Err(RpcError::internal("the job panicked")),
        Err(_) => Err(RpcError::cancelled()),
    };
    mgr.release(&entry);
    let cancelled = entry.cancel.is_cancelled();
    finish(&mgr, &entry, outcome, cancelled);
}

fn finish(inner: &Inner, entry: &JobEntry, outcome: Outcome, cancelled: bool) {
    let now = russet_core::time::now_rfc3339();
    let state = match &outcome {
        Ok(_) => JobState::Succeeded,
        Err(e) if cancelled || e.data.as_ref().is_some_and(|d| d.russet_code == codes::core::CANCELLED.as_str()) => {
            JobState::Cancelled
        }
        Err(_) => JobState::Failed,
    };
    {
        let mut info = lock(&entry.info);
        info.state = state;
        info.waiting_for = None;
        info.ended_at = Some(now.clone());
        if let Some(started) = info.started_at.as_deref().and_then(russet_core::time::parse_rfc3339) {
            info.duration_ms = SystemTime::now().duration_since(started).ok().map(russet_core::time::millis);
        }
        info.error = outcome.as_ref().err().and_then(|e| e.data.as_deref().cloned());
    }
    *lock(&entry.outcome) = Some(outcome);
    if let Some(dir) = &entry.dir {
        let r = {
            let info = lock(&entry.info).clone();
            match lock(&entry.outcome).as_ref() {
                Some(Ok(v)) => JobWaitResult { job: info, result: Some(v.clone()), error: None },
                Some(Err(e)) => JobWaitResult { job: info, result: None, error: e.data.as_deref().cloned() },
                None => JobWaitResult { job: info, result: None, error: None },
            }
        };
        if let Err(e) = write_result(dir, &r) {
            tracing::warn!("cannot write the result of {}: {}", entry.id, e.message());
        }
    }
    publish_info(inner, entry);
    // send_replace updates the value even when nobody is subscribed right now (a later `wait` reads it).
    entry.done.send_replace(true);
}

/// Locks taken with [`JobManager::lock`]; dropping releases them.
#[derive(Debug)]
pub struct HeldLocks {
    keys: Vec<LockKey>,
    inner: Weak<Inner>,
}

impl HeldLocks {
    /// The keys held.
    pub fn keys(&self) -> &[LockKey] {
        &self.keys
    }
}

impl Drop for HeldLocks {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            let mut s = lock(&inner.slots);
            for k in &self.keys {
                s.held.remove(k);
            }
            drop(s);
            inner.wake.notify_waiters();
        }
    }
}

/// A submitted job.
pub struct JobHandle {
    pub id: JobId,
    done: watch::Receiver<bool>,
    manager: JobManager,
}

impl std::fmt::Debug for JobHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobHandle").field("id", &self.id).finish()
    }
}

impl JobHandle {
    /// Waits for the job to end and returns its outcome (the JSON result, or the JSON-RPC error).
    pub async fn outcome(mut self) -> Outcome {
        let _ = self.done.wait_for(|d| *d).await;
        self.manager.outcome(&self.id).unwrap_or_else(|| Err(RpcError::internal("the job vanished")))
    }
}

/// A job's event sink (module docs).
pub struct JobEvents {
    entry: Arc<JobEntry>,
    inner: Weak<Inner>,
}

impl JobEvents {
    fn push(&self, kind: JobEventKind) {
        let event = {
            let mut log = lock(&self.entry.events);
            let event = JobEvent { seq: log.next_seq, ts: russet_core::time::now_rfc3339(), event: kind };
            log.next_seq += 1;
            if log.ring.len() == EVENT_RING {
                log.ring.pop_front();
            }
            log.ring.push_back(event.clone());
            event
        };
        for path in
            [self.entry.dir.as_ref().map(|d| d.join("events.ndjson")), self.entry.mirror.clone()].into_iter().flatten()
        {
            if let Err(e) = JsonLines::new(path).append(&event) {
                tracing::debug!("cannot persist a job event: {}", e.message());
            }
        }
        if let Some(inner) = self.inner.upgrade() {
            let _ = inner.updates.send(JobNotice::Event { job_id: self.entry.id.clone(), event });
        }
    }

    fn set_progress(&self, message: Option<&str>, percentage: Option<u8>) {
        let mut info = lock(&self.entry.info);
        let p = info.progress.get_or_insert_with(JobProgress::default);
        if let Some(m) = message {
            p.message = Some(m.to_owned());
        }
        if let Some(pct) = percentage {
            p.percentage = Some(pct.min(100));
        }
    }
}

impl ProgressSink for JobEvents {
    fn begin(&self, title: &str, _cancellable: bool) {
        self.set_progress(Some(title), None);
        self.push(JobEventKind::Progress { done: None, total: None, fraction: None, message: Some(title.to_owned()) });
    }

    fn report(&self, msg: Option<&str>, pct: Option<u8>) {
        self.set_progress(msg, pct);
        self.push(JobEventKind::Progress {
            done: None,
            total: None,
            fraction: pct.map(|p| f64::from(p.min(100)) / 100.0),
            message: msg.map(str::to_owned),
        });
    }

    fn end(&self, msg: Option<&str>) {
        self.set_progress(msg, Some(100));
        self.push(JobEventKind::Progress {
            done: None,
            total: None,
            fraction: Some(1.0),
            message: msg.map(str::to_owned),
        });
    }

    fn log(&self, stream: LogStream, line: &str) {
        self.push(JobEventKind::Log { stream, line: line.to_owned() });
    }
}

impl EventSink for JobEvents {
    fn emit(&self, e: JobEventKind) {
        self.push(e);
    }
}
