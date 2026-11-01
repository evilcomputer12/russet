// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Child processes (testability contract T1, ARCHITECTURE §5.3 `proc`).
//!
//! Contract [D 73]: [`ProcessRunner`] forwards every spawn to ONE long-lived `russet-spawner` OS thread (a
//! channel of spawn requests), because `PR_SET_PDEATHSIG` fires when the spawning *thread* exits. Spawning
//! from `spawn_blocking` threads (reaped after about 10 s idle) or from the device listen runtime would kill
//! children mid-build. The process-group kill (Linux) or Job Object (Windows) is the primary cleanup;
//! PDEATHSIG only covers a daemon crash.
//!
//! - Every child leads a new process group (Unix) or runs in a Job Object with `CREATE_NO_WINDOW` (Windows), so
//!   [`ChildProcess::kill_tree`] reaches its whole tree: `SIGTERM` to the group, [`KILL_GRACE`], then
//!   `SIGKILL`.
//! - On Linux the child also gets `PR_SET_PDEATHSIG(SIGKILL)` against the spawner thread, which lives as long as
//!   the process.
//! - stdin is closed; stdout and stderr are drained by background tasks, so a chatty child never blocks on a
//!   full pipe. [`CommandRunner::run`] streams their lines to the sink and keeps the last [`MAX_CAPTURE`] bytes
//!   of each.
//! - A non-zero exit is not an error here (the caller decides); cancellation is `core.cancelled` and a timeout
//!   `core.timeout`, both after the tree was killed.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use async_trait::async_trait;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, codes};
use crate::progress::{EventSink, LogStream};

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

impl CommandSpec {
    /// A spec for `program` with no arguments.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self { program: program.into(), ..Self::default() }
    }

    /// Appends an argument.
    pub fn arg(mut self, a: impl Into<OsString>) -> Self {
        self.args.push(a.into());
        self
    }

    /// Appends arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets an environment variable (for example from `ToolInfo.env`).
    pub fn env(mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> Self {
        self.env.insert(k.into(), v.into());
        self
    }

    /// Sets the working directory.
    pub fn cwd(mut self, d: impl Into<PathBuf>) -> Self {
        self.cwd = Some(d.into());
        self
    }

    /// Runs at a lower priority (`nice`; `BELOW_NORMAL_PRIORITY_CLASS` on Windows).
    pub fn nice(mut self, n: i32) -> Self {
        self.nice = Some(n);
        self
    }

    /// Kills the tree and fails with `core.timeout` after `d`.
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }
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

impl CommandOutput {
    /// Whether the process exited with code 0.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
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

/// How long a tree gets between `SIGTERM` and `SIGKILL` when it is killed.
pub const KILL_GRACE: Duration = Duration::from_millis(1000);

/// After the main process exits, how long its output pipes may stay open (a grandchild holding them) before
/// Russet stops reading.
pub const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Bytes of stdout and of stderr kept by [`CommandRunner::run`] and [`ChildProcess::wait`]; older output is
/// dropped (the sink still saw every line).
pub const MAX_CAPTURE: usize = 16 * 1024 * 1024;

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
        if cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        let mut child = spawn_wrapped(spec).await?;
        let (tx, mut rx) = mpsc::channel::<(LogStream, Vec<u8>)>(64);
        let readers = [
            child.stdout().take().map(|s| tokio::spawn(pump(s, LogStream::Stdout, tx.clone()))),
            child.stderr().take().map(|s| tokio::spawn(pump(s, LogStream::Stderr, tx.clone()))),
        ];
        drop(tx);
        let mut out = Capture::default();
        let mut err = Capture::default();
        let mut deliver = |stream: LogStream, chunk: Vec<u8>| {
            let (capture, label) = match stream {
                LogStream::Stdout => (&mut out, LogStream::Stdout),
                _ => (&mut err, LogStream::Stderr),
            };
            for line in capture.push(&chunk) {
                sink.log(label, &line);
            }
        };

        let outcome = {
            let wait = wait_outcome(child.as_mut(), cancel, spec.timeout);
            tokio::pin!(wait);
            loop {
                tokio::select! {
                    biased;
                    Some((stream, chunk)) = rx.recv() => deliver(stream, chunk),
                    o = &mut wait => break o,
                }
            }
        };
        let status = match outcome {
            Outcome::Exited(status) => status,
            Outcome::Cancelled | Outcome::TimedOut => {
                terminate_tree(child.as_mut(), KILL_GRACE).await;
                for r in readers.into_iter().flatten() {
                    r.abort();
                }
                return Err(if matches!(outcome, Outcome::Cancelled) {
                    Error::cancelled()
                } else {
                    Error::new(
                        codes::core::TIMEOUT,
                        format!("`{}` timed out after {:?}", spec.program.display(), spec.timeout.unwrap_or_default()),
                    )
                });
            }
            Outcome::Failed(e) => return Err(Error::io(format!("waiting for `{}`", spec.program.display()), e)),
        };
        // Drain what is left; a grandchild may hold the pipes open, so stop after DRAIN_GRACE.
        let deadline = tokio::time::sleep(DRAIN_GRACE);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                m = rx.recv() => match m {
                    Some((stream, chunk)) => deliver(stream, chunk),
                    None => break,
                },
                () = &mut deadline => {
                    for r in readers.iter().flatten() {
                        r.abort();
                    }
                    break;
                }
            }
        }
        for (capture, stream) in [(&mut out, LogStream::Stdout), (&mut err, LogStream::Stderr)] {
            if let Some(last) = capture.finish() {
                sink.log(stream, &last);
            }
        }
        Ok(output_of(status, out.bytes, err.bytes))
    }

    async fn spawn(&self, spec: &CommandSpec) -> crate::Result<Box<dyn ChildProcess>> {
        let mut child = spawn_wrapped(spec).await?;
        let pid = child.id();
        let stdout = child.stdout().take().map(|s| tokio::spawn(collect(s)));
        let stderr = child.stderr().take().map(|s| tokio::spawn(collect(s)));
        Ok(Box::new(ManagedChild {
            child,
            pid,
            program: spec.program.clone(),
            timeout: spec.timeout,
            stdout,
            stderr,
            exited: None,
        }))
    }
}

enum Outcome {
    Exited(ExitStatus),
    Cancelled,
    TimedOut,
    Failed(std::io::Error),
}

async fn wait_outcome(child: &mut dyn ChildWrapper, cancel: &CancellationToken, timeout: Option<Duration>) -> Outcome {
    let sleep = async {
        match timeout {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        r = child.wait() => match r {
            Ok(status) => Outcome::Exited(status),
            Err(e) => Outcome::Failed(e),
        },
        () = cancel.cancelled() => Outcome::Cancelled,
        () = sleep => Outcome::TimedOut,
    }
}

/// `SIGTERM` to the group, up to `grace` for the leader to exit, then `SIGKILL` to the group (Unix); the Job
/// Object is terminated on Windows. Stragglers that ignored `SIGTERM` die with the final `SIGKILL`.
async fn terminate_tree(child: &mut dyn ChildWrapper, grace: Duration) {
    #[cfg(unix)]
    {
        let _ = child.signal(nix::sys::signal::Signal::SIGTERM as i32);
        let exited = tokio::time::timeout(grace, child.wait()).await.is_ok();
        let _ = child.signal(nix::sys::signal::Signal::SIGKILL as i32);
        if !exited {
            let _ = child.wait().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = grace;
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

fn output_of(status: ExitStatus, stdout: Vec<u8>, stderr: Vec<u8>) -> CommandOutput {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    };
    #[cfg(not(unix))]
    let signal = None;
    CommandOutput { code: status.code(), signal, stdout, stderr }
}

/// Reads a pipe in chunks and forwards them.
async fn pump(mut r: impl AsyncRead + Unpin, stream: LogStream, tx: mpsc::Sender<(LogStream, Vec<u8>)>) {
    let mut buf = vec![0u8; 8192];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send((stream, buf[..n].to_vec())).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Reads a pipe to the end, keeping the last [`MAX_CAPTURE`] bytes.
async fn collect(mut r: impl AsyncRead + Unpin) -> Vec<u8> {
    let mut capture = Capture::default();
    let mut buf = vec![0u8; 8192];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                capture.push_bytes(&buf[..n]);
            }
        }
    }
    capture.bytes
}

/// Captured output plus the partial line not yet handed to a sink.
#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    partial: Vec<u8>,
}

impl Capture {
    fn push_bytes(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() > 2 * MAX_CAPTURE {
            let excess = self.bytes.len() - MAX_CAPTURE;
            self.bytes.drain(..excess);
        }
    }

    /// Appends `chunk` and returns the lines it completed (lossy UTF-8, without `\r\n`).
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.push_bytes(chunk);
        let mut lines = Vec::new();
        for &b in chunk {
            if b == b'\n' {
                lines.push(line_of(&self.partial));
                self.partial.clear();
            } else {
                self.partial.push(b);
            }
        }
        lines
    }

    /// The trailing line without a newline, if any; trims the capture to [`MAX_CAPTURE`].
    fn finish(&mut self) -> Option<String> {
        if self.bytes.len() > MAX_CAPTURE {
            let excess = self.bytes.len() - MAX_CAPTURE;
            self.bytes.drain(..excess);
        }
        if self.partial.is_empty() {
            return None;
        }
        let line = line_of(&self.partial);
        self.partial.clear();
        Some(line)
    }
}

fn line_of(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    s.strip_suffix('\r').unwrap_or(&s).to_owned()
}

/// A child from [`ProcessRunner::spawn`]; dropping it kills its tree.
struct ManagedChild {
    child: Box<dyn ChildWrapper>,
    pid: Option<u32>,
    program: PathBuf,
    timeout: Option<Duration>,
    stdout: Option<JoinHandle<Vec<u8>>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    exited: Option<ExitStatus>,
}

impl ManagedChild {
    async fn collected(&mut self) -> (Vec<u8>, Vec<u8>) {
        async fn join(h: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
            let Some(mut h) = h else { return Vec::new() };
            match tokio::time::timeout(DRAIN_GRACE, &mut h).await {
                Ok(Ok(bytes)) => bytes,
                _ => {
                    h.abort();
                    Vec::new()
                }
            }
        }
        (join(self.stdout.take()).await, join(self.stderr.take()).await)
    }
}

#[async_trait]
impl ChildProcess for ManagedChild {
    fn id(&self) -> Option<u32> {
        if self.exited.is_some() { None } else { self.pid }
    }

    async fn wait(&mut self) -> crate::Result<CommandOutput> {
        let status = match self.exited {
            Some(s) => s,
            None => {
                let never = CancellationToken::new();
                match wait_outcome(self.child.as_mut(), &never, self.timeout).await {
                    Outcome::Exited(s) => s,
                    Outcome::Failed(e) => {
                        return Err(Error::io(format!("waiting for `{}`", self.program.display()), e));
                    }
                    Outcome::TimedOut | Outcome::Cancelled => {
                        terminate_tree(self.child.as_mut(), KILL_GRACE).await;
                        self.exited = Some(self.child.wait().await.map_err(|e| Error::io("reaping a child", e))?);
                        return Err(Error::new(
                            codes::core::TIMEOUT,
                            format!(
                                "`{}` timed out after {:?}",
                                self.program.display(),
                                self.timeout.unwrap_or_default()
                            ),
                        ));
                    }
                }
            }
        };
        self.exited = Some(status);
        let (out, err) = self.collected().await;
        Ok(output_of(status, out, err))
    }

    async fn kill_tree(&mut self) -> crate::Result<()> {
        if self.exited.is_none() {
            terminate_tree(self.child.as_mut(), KILL_GRACE).await;
            self.exited = Some(self.child.wait().await.map_err(|e| Error::io("reaping a killed child", e))?);
        } else {
            // The leader is gone; make sure no straggler of its group survives.
            #[cfg(unix)]
            let _ = self.child.signal(nix::sys::signal::Signal::SIGKILL as i32);
        }
        Ok(())
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if self.exited.is_none() {
            let _ = self.child.start_kill();
        }
        for h in [self.stdout.take(), self.stderr.take()].into_iter().flatten() {
            h.abort();
        }
    }
}

// ---- the spawner thread [D 73] -------------------------------------------------------------------------

struct SpawnRequest {
    wrap: CommandWrap,
    runtime: tokio::runtime::Handle,
    reply: oneshot::Sender<std::io::Result<Box<dyn ChildWrapper>>>,
}

fn spawner() -> &'static std_mpsc::Sender<SpawnRequest> {
    static SPAWNER: OnceLock<std_mpsc::Sender<SpawnRequest>> = OnceLock::new();
    SPAWNER.get_or_init(|| {
        let (tx, rx) = std_mpsc::channel::<SpawnRequest>();
        let started = std::thread::Builder::new().name("russet-spawner".to_owned()).spawn(move || {
            for mut req in rx {
                // The child registers with the caller's runtime (its reaper and pipes); the fork happens here,
                // on a thread that lives as long as the process, so PDEATHSIG only fires when the process dies.
                let _guard = req.runtime.enter();
                let result = req.wrap.spawn();
                let _ = req.reply.send(result);
            }
        });
        if let Err(e) = started {
            // Without the spawner no child can be started; every spawn then fails with this error.
            tracing::error!("cannot start the russet-spawner thread: {e}");
        }
        tx
    })
}

/// Builds the wrapped command for `spec` and spawns it on the spawner thread.
async fn spawn_wrapped(spec: &CommandSpec) -> crate::Result<Box<dyn ChildWrapper>> {
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|_| Error::internal("child processes can only be spawned inside a tokio runtime"))?;
    let wrap = build(spec);
    let (reply, rx) = oneshot::channel();
    spawner()
        .send(SpawnRequest { wrap, runtime, reply })
        .map_err(|_| Error::internal("the russet-spawner thread is gone"))?;
    let result = rx.await.map_err(|_| Error::internal("the russet-spawner thread dropped a spawn request"))?;
    result.map_err(|e| spawn_error(spec, e))
}

fn spawn_error(spec: &CommandSpec, e: std::io::Error) -> Error {
    let program = spec.program.display();
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            Error::new(codes::core::NOT_FOUND, format!("cannot run `{program}`: not found"))
                .with_hint("Install it, or check PATH and the toolchain settings (`russet doctor`).")
                .caused_by(e)
        }
        std::io::ErrorKind::PermissionDenied => {
            Error::new(codes::core::IO, format!("cannot run `{program}`: permission denied"))
                .with_hint("Check that the file is executable.")
                .caused_by(e)
        }
        _ => Error::io(format!("cannot run `{program}`"), e),
    }
}

fn build(spec: &CommandSpec) -> CommandWrap {
    let mut wrap = CommandWrap::with_new(&spec.program, |cmd| {
        cmd.args(&spec.args);
        if spec.env_clear {
            cmd.env_clear();
        }
        cmd.envs(&spec.env);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    });
    #[cfg(unix)]
    {
        wrap.wrap(process_wrap::tokio::ProcessGroup::leader());
        wrap.wrap(unix::ChildSetup { nice: spec.nice, parent: std::process::id() });
    }
    #[cfg(windows)]
    {
        use windows::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW};
        let mut flags = CREATE_NO_WINDOW;
        if spec.nice.is_some_and(|n| n > 0) {
            flags |= BELOW_NORMAL_PRIORITY_CLASS;
        }
        wrap.wrap(process_wrap::tokio::CreationFlags(flags));
        wrap.wrap(process_wrap::tokio::JobObject);
    }
    wrap.wrap(KillOnDrop);
    wrap
}

#[cfg(unix)]
mod unix {
    use process_wrap::tokio::{CommandWrap, CommandWrapper};

    /// Runs in the forked child before `exec`: lower the priority, and on Linux ask for `SIGKILL` when the
    /// spawner thread (that is, the process) dies.
    #[derive(Debug, Clone, Copy)]
    pub(super) struct ChildSetup {
        pub(super) nice: Option<i32>,
        pub(super) parent: u32,
    }

    impl CommandWrapper for ChildSetup {
        fn pre_spawn(&mut self, command: &mut tokio::process::Command, _core: &CommandWrap) -> std::io::Result<()> {
            let nice = self.nice;
            #[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
            let parent = self.parent;
            #[allow(unsafe_code)]
            // SAFETY: the closure runs between fork and exec, where only async-signal-safe calls are allowed. It
            // calls setpriority(2), prctl(2) and getppid(2), which are, and it allocates nothing: the error path
            // builds an io::Error from a raw OS error, which stores the integer only.
            unsafe {
                command.pre_exec(move || {
                    if let Some(n) = nice {
                        // A failure to lower the priority is harmless; never fail the spawn for it.
                        let _ = nix::libc::setpriority(nix::libc::PRIO_PROCESS, 0, n);
                    }
                    #[cfg(target_os = "linux")]
                    {
                        nix::sys::prctl::set_pdeathsig(nix::sys::signal::Signal::SIGKILL)
                            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
                        // The parent may have died between fork and prctl: then nobody would send the signal.
                        if nix::unistd::getppid().as_raw() as u32 != parent {
                            return Err(std::io::Error::from_raw_os_error(nix::libc::ESRCH));
                        }
                    }
                    Ok(())
                });
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::progress::{JobEventKind, ProgressSink};

    #[derive(Default)]
    struct Lines(Mutex<Vec<(LogStream, String)>>);

    impl ProgressSink for Lines {
        fn begin(&self, _: &str, _: bool) {}
        fn report(&self, _: Option<&str>, _: Option<u8>) {}
        fn end(&self, _: Option<&str>) {}
        fn log(&self, stream: LogStream, line: &str) {
            self.0.lock().expect("lock").push((stream, line.to_owned()));
        }
    }

    impl EventSink for Lines {
        fn emit(&self, _e: JobEventKind) {}
    }

    fn sh(script: &str) -> CommandSpec {
        CommandSpec::new("/bin/sh").arg("-c").arg(script)
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "needs Unix processes and a shell; the engine runs in Linux (WSL on Windows)")]
    async fn run_streams_lines_and_captures_output() {
        let sink = Lines::default();
        let out = ProcessRunner
            .run(&sh("echo one; echo two >&2; printf three; exit 3"), &sink, &CancellationToken::new())
            .await
            .expect("runs");
        assert_eq!(out.code, Some(3));
        assert!(!out.success());
        assert_eq!(out.stdout, b"one\nthree");
        assert_eq!(out.stderr, b"two\n");
        let lines = sink.0.lock().expect("lock").clone();
        assert!(lines.contains(&(LogStream::Stdout, "one".to_owned())));
        assert!(lines.contains(&(LogStream::Stdout, "three".to_owned())));
        assert!(lines.contains(&(LogStream::Stderr, "two".to_owned())));
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "needs Unix processes and a shell; the engine runs in Linux (WSL on Windows)")]
    async fn env_cwd_and_env_clear_apply() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let spec = sh("echo \"$RUSSET_T:$HOME:$(pwd)\"").env("RUSSET_T", "v").cwd(tmp.path());
        let mut cleared = spec.clone();
        cleared.env_clear = true;
        let out =
            ProcessRunner.run(&cleared, &crate::progress::NullSink, &CancellationToken::new()).await.expect("runs");
        let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let expected_dir = tmp.path().canonicalize().expect("canonical");
        assert_eq!(text, format!("v::{}", expected_dir.display()));
    }

    #[tokio::test]
    async fn missing_programs_are_not_found() {
        let e = ProcessRunner
            .run(
                &CommandSpec::new("/nonexistent/russet-test-binary"),
                &crate::progress::NullSink,
                &CancellationToken::new(),
            )
            .await
            .expect_err("missing");
        assert_eq!(e.code(), codes::core::NOT_FOUND);
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "needs Unix processes and a shell; the engine runs in Linux (WSL on Windows)")]
    async fn cancellation_and_timeouts_kill_the_tree() {
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            c2.cancel();
        });
        let started = std::time::Instant::now();
        let e = ProcessRunner.run(&sh("sleep 30"), &crate::progress::NullSink, &cancel).await.expect_err("cancelled");
        assert_eq!(e.code(), codes::core::CANCELLED);
        assert!(started.elapsed() < Duration::from_secs(5));

        let e = ProcessRunner
            .run(
                &sh("sleep 30").timeout(Duration::from_millis(200)),
                &crate::progress::NullSink,
                &CancellationToken::new(),
            )
            .await
            .expect_err("times out");
        assert_eq!(e.code(), codes::core::TIMEOUT);
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "needs Unix processes and a shell; the engine runs in Linux (WSL on Windows)")]
    async fn spawned_children_wait_and_kill() {
        let mut child = ProcessRunner.spawn(&sh("echo hi; exit 4")).await.expect("spawns");
        assert!(child.id().is_some());
        let out = child.wait().await.expect("waits");
        assert_eq!((out.code, out.stdout.as_slice()), (Some(4), b"hi\n".as_slice()));
        assert!(child.id().is_none());

        let mut long = ProcessRunner.spawn(&sh("sleep 30")).await.expect("spawns");
        long.kill_tree().await.expect("kills");
        let out = long.wait().await.expect("reaped");
        assert!(out.signal.is_some() || out.code.is_some_and(|c| c != 0), "{out:?}");
    }

    #[test]
    fn capture_splits_lines_across_chunks() {
        let mut c = Capture::default();
        assert!(c.push(b"ab").is_empty());
        assert_eq!(c.push(b"c\r\nd"), vec!["abc".to_owned()]);
        assert_eq!(c.finish(), Some("d".to_owned()));
        assert_eq!(c.bytes, b"abc\r\nd");
    }
}
