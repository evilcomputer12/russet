// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Running probe commands (`swift --version`, `lldb -b -o 'platform list'`): short-lived children with a timeout,
//! killed when the timeout fires. [`Exec`] is the seam tests replace.
//!
//! [`RunnerExec`] runs probes through russet-core's [`CommandRunner`], the single `russet-spawner` thread of
//! [D 73]; the daemon uses it through [`Toolchains::with_runner`](crate::Toolchains::with_runner) once W2's
//! `ProcessRunner` (F1) is on main. Until then the default is [`ProcessExec`] (`tokio::process`; probes set no
//! `PR_SET_PDEATHSIG`, so the spawning thread's exit cannot kill them).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russet_core::error::{Error, Result, codes};
use russet_core::proc::{CommandRunner, CommandSpec};
use russet_core::progress::{EventSink, JobEventKind, LogStream, ProgressSink};
use tokio_util::sync::CancellationToken;

/// What a probe printed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutput {
    /// The exit code; `None` when the process was killed by a signal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl ExecOutput {
    /// Whether the process exited with status 0.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// stdout followed by stderr.
    pub fn combined(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// Runs a probe command.
#[async_trait]
pub trait Exec: Send + Sync {
    /// Runs `program args…` with `env` added to the inherited environment; fails after `timeout`.
    async fn output(
        &self,
        program: &Path,
        args: &[&str],
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<ExecOutput>;
}

/// The real runner: `tokio::process` with `kill_on_drop`, stdin closed.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessExec;

#[async_trait]
impl Exec for ProcessExec {
    async fn output(
        &self,
        program: &Path,
        args: &[&str],
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<ExecOutput> {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args).envs(env).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| {
            Error::new(codes::toolchain::BROKEN, format!("cannot run {}: {e}", program.display()))
                .with_hint("Check that the file is executable for this architecture.")
                .caused_by(e)
        })?;
        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => Ok(ExecOutput {
                code: out.status.code(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
            Ok(Err(e)) => Err(Error::new(codes::toolchain::BROKEN, format!("{} failed: {e}", program.display()))
                .with_hint("Run it by hand to see why.")
                .caused_by(e)),
            Err(_) => Err(Error::new(
                codes::core::TIMEOUT,
                format!("{} did not answer within {} ms", program.display(), timeout.as_millis()),
            )
            .with_hint("The tool hangs or the machine is overloaded; run it by hand.")),
        }
    }
}

/// Probes through a [`CommandRunner`]: the program with `args`, `env` added to the inherited environment, stdin
/// closed and `timeout` as the spec's timeout (the runner kills the tree and answers `core.timeout`).
pub struct RunnerExec {
    runner: Arc<dyn CommandRunner>,
}

impl std::fmt::Debug for RunnerExec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerExec").finish_non_exhaustive()
    }
}

impl RunnerExec {
    /// Probes through `runner`.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

/// Probe output is parsed, not streamed: the runner's progress and log events go nowhere.
struct Discard;

impl ProgressSink for Discard {
    fn begin(&self, _title: &str, _cancellable: bool) {}
    fn report(&self, _msg: Option<&str>, _pct: Option<u8>) {}
    fn end(&self, _msg: Option<&str>) {}
    fn log(&self, _stream: LogStream, _line: &str) {}
}

impl EventSink for Discard {
    fn emit(&self, _e: JobEventKind) {}
}

#[async_trait]
impl Exec for RunnerExec {
    async fn output(
        &self,
        program: &Path,
        args: &[&str],
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<ExecOutput> {
        let spec = CommandSpec {
            program: program.to_path_buf(),
            args: args.iter().map(OsString::from).collect(),
            env: env.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect(),
            timeout: Some(timeout),
            ..CommandSpec::default()
        };
        let out = self.runner.run(&spec, &Discard, &CancellationToken::new()).await?;
        Ok(ExecOutput {
            code: out.code,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// One cached probe result.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CacheEntry {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Caches probe output in `cache/toolchain-probes.json`, keyed by the program's path, modification time and size,
/// the arguments and the environment (core §10.4). `refresh` drops everything.
pub struct CachedExec {
    inner: std::sync::Arc<dyn Exec>,
    file: Option<std::path::PathBuf>,
    entries: std::sync::Mutex<BTreeMap<String, CacheEntry>>,
    dirty: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for CachedExec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedExec").field("file", &self.file).finish_non_exhaustive()
    }
}

impl CachedExec {
    /// A cache over `inner`, persisted in `file` when given.
    pub fn new(inner: std::sync::Arc<dyn Exec>, file: Option<std::path::PathBuf>) -> Self {
        let entries = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self { inner, file, entries: std::sync::Mutex::new(entries), dirty: std::sync::atomic::AtomicBool::new(false) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, CacheEntry>> {
        match self.entries.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    /// Drops every cached result.
    pub fn clear(&self) {
        self.lock().clear();
        if let Some(f) = &self.file {
            let _ = std::fs::remove_file(f);
        }
    }

    fn key(program: &Path, args: &[&str], env: &BTreeMap<String, String>) -> Option<String> {
        let real = std::fs::canonicalize(program).ok()?;
        let meta = std::fs::metadata(&real).ok()?;
        let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
        Some(format!("{}|{mtime}|{}|{}|{env:?}", real.display(), meta.len(), args.join("\u{1f}")))
    }

    /// Writes the cache file if a probe added results since the last flush (the manager calls it once per
    /// operation, outside the lock, rather than once per probe).
    pub fn flush(&self) {
        if !self.dirty.swap(false, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        let Some(f) = &self.file else { return };
        let snapshot = self.lock().clone();
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_vec(&snapshot) {
            let tmp = f.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, f);
            }
        }
    }
}

#[async_trait]
impl Exec for CachedExec {
    async fn output(
        &self,
        program: &Path,
        args: &[&str],
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<ExecOutput> {
        let key = Self::key(program, args, env);
        if let Some(k) = &key
            && let Some(e) = self.lock().get(k)
        {
            return Ok(ExecOutput { code: e.code, stdout: e.stdout.clone(), stderr: e.stderr.clone() });
        }
        let out = self.inner.output(program, args, env, timeout).await?;
        if let Some(k) = key {
            self.lock()
                .insert(k, CacheEntry { code: out.code, stdout: out.stdout.clone(), stderr: out.stderr.clone() });
            self.dirty.store(true, std::sync::atomic::Ordering::Release);
        }
        Ok(out)
    }
}
