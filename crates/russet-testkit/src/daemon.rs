// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`SpawnedDaemon`]: the temp-home daemon fixture (CORE-15): `russet daemon run` in a [`TempHome`], with
//! readiness, logs and teardown.
//!
//! The daemon runs in the foreground as a child of the test, with the home's environment (lane D: memory
//! secrets, a private live-device lock, the scripted backend; never `RUSSET_LANE`). It is ready when its socket
//! (`$RUSSET_HOME/runtime/russetd.sock`, ARCHITECTURE §7.1) accepts a connection. Its stdout and stderr go to
//! `daemon.log` in the home, which [`SpawnedDaemon::log`] returns and every error quotes. Dropping the fixture
//! kills the daemon.
//!
//! In-process daemons (`russetd::InProcess::start`) need no fixture: pass [`TempHome::dirs`] and
//! [`TempHome::socket_path`] to `DaemonOptions`.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::home::TempHome;

/// How long a daemon may take to accept connections (ARCHITECTURE §3.3: clients poll up to 10 s).
pub const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// A daemon process running in a [`TempHome`].
#[derive(Debug)]
pub struct SpawnedDaemon {
    child: Option<Child>,
    socket: PathBuf,
    log: PathBuf,
}

impl SpawnedDaemon {
    /// Starts `<exe> daemon run [extra…]` in `home` and waits until its socket accepts connections.
    pub fn spawn(home: &TempHome, exe: &Path, extra: &[&str]) -> io::Result<Self> {
        let log = home.root().join("daemon.log");
        let out = File::create(&log)?;
        let err = out.try_clone()?;
        let mut cmd = Command::new(exe);
        cmd.args(["daemon", "run"]).args(extra).current_dir(home.root());
        home.apply(&mut cmd);
        cmd.stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
        let child = cmd.spawn().map_err(|e| io::Error::new(e.kind(), format!("starting {}: {e}", exe.display())))?;
        let mut d = Self { child: Some(child), socket: home.socket_path(), log };
        d.wait_ready(READY_TIMEOUT)?;
        Ok(d)
    }

    fn wait_ready(&mut self, timeout: Duration) -> io::Result<()> {
        let start = Instant::now();
        loop {
            if let Some(child) = self.child.as_mut()
                && let Some(status) = child.try_wait()?
            {
                return Err(io::Error::other(format!(
                    "the daemon exited with {status} before it was ready; its log:\n{}",
                    self.log()
                )));
            }
            if connectable(&self.socket) {
                return Ok(());
            }
            if start.elapsed() > timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("the daemon did not accept connections within {timeout:?}; its log:\n{}", self.log()),
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The daemon's socket.
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// What the daemon wrote to stdout and stderr so far.
    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Kills the daemon and waits for it.
    pub fn stop(mut self) -> io::Result<ExitStatus> {
        let mut child = self.child.take().ok_or_else(|| io::Error::other("already stopped"))?;
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        child.wait()
    }

    /// Connects a tokio Unix stream to the daemon.
    #[cfg(unix)]
    pub async fn connect(&self) -> io::Result<tokio::net::UnixStream> {
        tokio::net::UnixStream::connect(&self.socket).await
    }
}

impl Drop for SpawnedDaemon {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(unix)]
fn connectable(socket: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

#[cfg(not(unix))]
fn connectable(_socket: &Path) -> bool {
    // Windows daemons listen on a named pipe (ARCHITECTURE §7.1); W27a brings the pipe fixture.
    false
}

/// The `russet` binary for spawned-daemon tests: `RUSSET_TEST_BIN` when set; otherwise this worktree's own
/// build, made by `cargo build --locked -p russet-cli --bin russet` once per test process (a no-op when it is
/// fresh). A cargo target slot shared by several worktrees (AGENTS.md §6) can hold another worktree's `russet` at
/// `target/<profile>/russet`; asking cargo from this workspace rebuilds it from this worktree's sources first
/// (the `RUSSET_BUILD_WORKTREE` guard), so a test never spawns another branch's daemon. Cargo releases the build
/// lock before it runs tests, so the nested build only waits for builds of other worktrees in the slot.
pub fn russet_bin() -> io::Result<PathBuf> {
    if let Some(p) = std::env::var_os("RUSSET_TEST_BIN") {
        return Ok(PathBuf::from(p));
    }
    static BUILT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    BUILT.get_or_init(build_russet).clone().map_err(io::Error::other)
}

/// `cargo build -p russet-cli --bin russet` in this workspace, with the test binary's profile; the executable
/// cargo reports.
fn build_russet() -> Result<PathBuf, String> {
    let workspace = crate::testkit_dir()
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "the testkit is not inside a workspace".to_string())?;
    // Cargo sets CARGO for the tests it runs; nextest passes it on.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(workspace).args([
        "build",
        "--locked",
        "--package",
        "russet-cli",
        "--bin",
        "russet",
        "--message-format",
        "json-render-diagnostics",
    ]);
    if !cfg!(debug_assertions) {
        cmd.arg("--release");
    }
    let out = cmd
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("running cargo build -p russet-cli: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo build -p russet-cli --bin russet failed with {}", out.status));
    }
    russet_executable(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "cargo build -p russet-cli named no `russet` executable".to_string())
}

/// The `russet` executable in cargo's JSON messages (`compiler-artifact` of the `russet` bin target).
pub fn russet_executable(messages: &str) -> Option<PathBuf> {
    messages.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find_map(|m| {
        let is_bin = m["target"]["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "bin"));
        (m["reason"] == "compiler-artifact" && m["target"]["name"] == "russet" && is_bin)
            .then(|| m["executable"].as_str().map(PathBuf::from))
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_russet_executable_is_read_from_cargo_messages() {
        let messages = [
            r#"{"reason":"compiler-artifact","target":{"name":"russet_cli","kind":["lib"]},"executable":null}"#,
            "not json",
            r#"{"reason":"compiler-artifact","target":{"name":"russet","kind":["bin"]},"executable":"/t/debug/russet"}"#,
            r#"{"reason":"build-finished","success":true}"#,
        ]
        .join("\n");
        assert_eq!(russet_executable(&messages), Some(PathBuf::from("/t/debug/russet")));
        assert_eq!(russet_executable(r#"{"reason":"build-finished","success":true}"#), None);
        let fresh = r#"{"reason":"compiler-artifact","target":{"name":"russet","kind":["bin"]},"executable":"/t/r","fresh":true}"#;
        assert_eq!(russet_executable(fresh), Some(PathBuf::from("/t/r")), "a fresh artifact is reported too");
    }
}
