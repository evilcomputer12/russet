// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Per-method job declarations (ARCHITECTURE §6.2): the locks, weight, event mirror, kind and title of the job a
//! long-running call runs as.
//!
//! The daemon layer runs every long-running method as a job ([`super::layer`]); what that job locks depends on
//! the call. The area that owns a method declares it with [`declare`] from its `handlers/<ns>.rs::register`
//! (the handler seam): `build.start` locks its project, `app.install` the device its selector resolves to
//! (the declaration gets the [`DaemonCtx`], so it can ask the device plane), `sdk.import` the SDK store. The
//! layer evaluates the declaration before it submits the job; an error answers the call and creates no job.
//! Params that do not fit the method's type leave the call to [`defaults`], and the handler reports them.
//!
//! [`defaults`] covers the rules of §6.2 that the params alone decide, for methods nobody declared:
//! `build.start` locks `LockKey::Project(project)`, counts against `daemon.max-build-jobs` and mirrors its events
//! to `<project>/.russet/logs/<job-id>.jsonl`; `sdk.import` locks `LockKey::Sdk`. Every job records the
//! `project` and `udid` params it names.
//!
//! Methods that are not long-running but still need a lock (`app.uninstall`, `sdk.remove`) take it with
//! [`JobManager::lock`](super::JobManager::lock).

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use russet_core::error::Result;
use russet_ipc::{BoxFuture, Router};
use russet_protocol::RpcMethod;
use serde_json::Value;

use super::LockKey;
use crate::DaemonCtx;

/// What a long-running call's job looks like beyond its defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobOptions {
    /// The job kind (`build`, `install`, …; ARCHITECTURE §6.2); default: the method name.
    pub kind: Option<String>,
    /// A human title (`Build RussetProbe`); default: the method name.
    pub title: Option<String>,
    /// Locks held while the job runs; it waits (`queued`, `waitingFor`) while another job holds one.
    pub locks: Vec<LockKey>,
    /// Counts against `daemon.max-build-jobs`.
    pub heavy: bool,
    /// Also append every event to `<dir>/<job-id>.jsonl` (build jobs: `<project>/.russet/logs`).
    pub mirror_dir: Option<PathBuf>,
    /// The project root the job is about (`job.list --project`, `JobInfo.project`).
    pub project: Option<PathBuf>,
    /// The device (UDID) the job is about (`JobInfo.device`).
    pub device: Option<String>,
}

impl JobOptions {
    /// No locks, not heavy, no mirror.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the job kind.
    #[must_use]
    pub fn kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Sets the title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Adds a lock.
    #[must_use]
    pub fn lock(mut self, key: LockKey) -> Self {
        if !self.locks.contains(&key) {
            self.locks.push(key);
        }
        self
    }

    /// Counts the job against `daemon.max-build-jobs`.
    #[must_use]
    pub fn heavy(mut self, heavy: bool) -> Self {
        self.heavy = heavy;
        self
    }

    /// Mirrors the job's events to `<dir>/<job-id>.jsonl`.
    #[must_use]
    pub fn mirror_to(mut self, dir: impl Into<PathBuf>) -> Self {
        self.mirror_dir = Some(dir.into());
        self
    }

    /// Records the project root.
    #[must_use]
    pub fn project(mut self, root: impl Into<PathBuf>) -> Self {
        self.project = Some(root.into());
        self
    }

    /// Records the device.
    #[must_use]
    pub fn device(mut self, udid: impl Into<String>) -> Self {
        self.device = Some(udid.into());
        self
    }

    /// A build of `project`: the project lock, the build slot and the event mirror of ARCHITECTURE §6.2.
    pub fn build_of(project: &Path) -> Self {
        let root = normalized(project);
        Self::new()
            .lock(LockKey::Project(root.clone()))
            .heavy(true)
            .mirror_to(root.join(".russet").join("logs"))
            .project(root)
    }
}

/// A project root as a lock key: `.` components and trailing separators do not make two keys.
fn normalized(p: &Path) -> PathBuf {
    p.components().collect()
}

type DeclareFn = Arc<dyn Fn(DaemonCtx, Value) -> BoxFuture<Result<Option<JobOptions>>> + Send + Sync>;

/// The declarations made with [`declare`] (a router extension until the daemon takes it).
#[derive(Clone, Default)]
pub struct JobDeclarations {
    by_method: BTreeMap<&'static str, DeclareFn>,
}

impl std::fmt::Debug for JobDeclarations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.by_method.keys()).finish()
    }
}

impl JobDeclarations {
    /// The options of a call of `method`: its declaration, else [`defaults`].
    pub async fn options(&self, cx: &DaemonCtx, method: &str, params: &Value) -> Result<JobOptions> {
        if let Some(declared) = self.by_method.get(method)
            && let Some(options) = declared(cx.clone(), params.clone()).await?
        {
            return Ok(options);
        }
        Ok(defaults(method, params))
    }

    /// Whether `method` has a declaration.
    pub fn declares(&self, method: &str) -> bool {
        self.by_method.contains_key(method)
    }
}

/// Declares the job options of long-running method `M` (module docs). The last declaration wins.
pub fn declare<M, F, Fut>(r: &mut Router<DaemonCtx>, f: F)
where
    M: RpcMethod,
    F: Fn(DaemonCtx, M::Params) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<JobOptions>> + Send + 'static,
{
    debug_assert!(M::INFO.long_running, "{} is not long-running, so it never runs as a job", M::NAME);
    let f = Arc::new(f);
    let erased: DeclareFn = Arc::new(move |cx, params| {
        let f = Arc::clone(&f);
        Box::pin(async move {
            match serde_json::from_value::<M::Params>(params) {
                Ok(p) => f(cx, p).await.map(Some),
                Err(_) => Ok(None),
            }
        })
    });
    r.extensions_mut().get_or_default::<JobDeclarations>().by_method.insert(M::NAME, erased);
}

/// The options of a call nobody declared (module docs).
pub fn defaults(method: &str, params: &Value) -> JobOptions {
    let project = params.get("project").and_then(Value::as_str).filter(|p| !p.is_empty()).map(Path::new);
    let mut o = match (method, project) {
        ("build.start", Some(root)) => JobOptions::build_of(root),
        ("build.start", None) => JobOptions::new().heavy(true),
        ("sdk.import", _) => JobOptions::new().lock(LockKey::Sdk),
        _ => JobOptions::new(),
    };
    if o.project.is_none() {
        o.project = project.map(normalized);
    }
    o.device = params.get("udid").and_then(Value::as_str).map(str::to_owned);
    o
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_defaults_follow_section_6_2() {
        let b = defaults("build.start", &json!({"project": "/w/App/"}));
        assert_eq!(b.locks, vec![LockKey::Project(PathBuf::from("/w/App"))]);
        assert!(b.heavy);
        assert_eq!(b.mirror_dir, Some(PathBuf::from("/w/App/.russet/logs")));
        assert_eq!(b.project, Some(PathBuf::from("/w/App")));
        assert_eq!(defaults("build.start", &json!({"project": "/w/./App"})).locks, b.locks, "one project, one key");
        let s = defaults("sdk.import", &json!({"source": "/x.xip"}));
        assert_eq!(s.locks, vec![LockKey::Sdk]);
        let i = defaults("toolchain.install", &json!({"udid": "u1", "project": "/p"}));
        assert!(i.locks.is_empty() && !i.heavy && i.mirror_dir.is_none());
        assert_eq!((i.device.as_deref(), i.project), (Some("u1"), Some(PathBuf::from("/p"))));
    }
}
