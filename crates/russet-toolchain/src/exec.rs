// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Running probe commands (`swift --version`, `lldb -b -o 'platform list'`): short-lived children with a timeout,
//! killed when the timeout fires. [`Exec`] is the seam tests replace.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use russet_core::error::{Error, Result, codes};

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
        Self { inner, file, entries: std::sync::Mutex::new(entries) }
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

    fn persist(&self, entries: &BTreeMap<String, CacheEntry>) {
        if let Some(f) = &self.file {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(text) = serde_json::to_vec(entries) {
                let tmp = f.with_extension("json.tmp");
                if std::fs::write(&tmp, text).is_ok() {
                    let _ = std::fs::rename(&tmp, f);
                }
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
            let mut entries = self.lock();
            entries.insert(k, CacheEntry { code: out.code, stdout: out.stdout.clone(), stderr: out.stderr.clone() });
            self.persist(&entries);
        }
        Ok(out)
    }
}
