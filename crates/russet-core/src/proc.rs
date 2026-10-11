// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Child processes (testability contract T1, ARCHITECTURE §5.3 `proc`).
//!
//! Contract [D 73]: [`ProcessRunner`] forwards every spawn to ONE long-lived `russet-spawner` OS thread (a
//! channel of spawn requests), because `PR_SET_PDEATHSIG` fires when the spawning *thread* exits. Spawning
//! from `spawn_blocking` threads (reaped after about 10 s idle) or from the device listen runtime would kill
//! children mid-build. The process-group kill (Linux) or Job Object (Windows) is the primary cleanup;
//! PDEATHSIG only covers a daemon crash.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::progress::EventSink;

/// What to run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub env_clear: bool,
    pub cwd: Option<PathBuf>,
    pub nice: Option<i32>,
    pub timeout: Option<Duration>,
}

/// The result of a finished command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutput {
    /// Exit code, when the process exited normally.
    pub code: Option<i32>,
    /// Terminating signal (Unix), when the process was killed.
    pub signal: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// A spawned child, killed with its whole process tree.
#[async_trait]
pub trait ChildProcess: Send + Sync {
    /// The OS process id, while the process runs.
    fn id(&self) -> Option<u32>;
    /// Waits for exit.
    async fn wait(&mut self) -> crate::Result<CommandOutput>;
    /// Kills the process group (Linux) or the Job Object (Windows).
    async fn kill_tree(&mut self) -> crate::Result<()>;
}

/// Runs and spawns child processes. Test doubles (cassette, recording, scripted runners) live in
/// `russet-testkit`.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    /// Runs to completion, streaming output lines to `sink`; `cancel` kills the process tree.
    async fn run(
        &self,
        spec: &CommandSpec,
        sink: &dyn EventSink,
        cancel: &CancellationToken,
    ) -> crate::Result<CommandOutput>;
    /// Spawns without waiting.
    async fn spawn(&self, spec: &CommandSpec) -> crate::Result<Box<dyn ChildProcess>>;
}

/// The real runner: process-wrap with a process group and PDEATHSIG (Linux), a Job Object and
/// `CREATE_NO_WINDOW` (Windows), all spawned from the single `russet-spawner` thread.
#[derive(Debug, Clone, Default)]
pub struct ProcessRunner;

#[async_trait]
impl CommandRunner for ProcessRunner {
    async fn run(
        &self,
        spec: &CommandSpec,
        sink: &dyn EventSink,
        cancel: &CancellationToken,
    ) -> crate::Result<CommandOutput> {
        todo!("W2 (CORE-02): process-wrap behind the russet-spawner thread [D 73]")
    }

    async fn spawn(&self, spec: &CommandSpec) -> crate::Result<Box<dyn ChildProcess>> {
        todo!("W2 (CORE-02): process-wrap behind the russet-spawner thread [D 73]")
    }
}
