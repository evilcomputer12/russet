// SPDX-License-Identifier: GPL-3.0-or-later
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

/// The `russet` binary for spawned-daemon tests: `RUSSET_TEST_BIN`, else `russet` next to the test binary's
/// target directory (`target/<profile>/russet`), built by `cargo build -p russet-cli`.
pub fn russet_bin() -> io::Result<PathBuf> {
    if let Some(p) = std::env::var_os("RUSSET_TEST_BIN") {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe()?;
    // target/<profile>/deps/<test>-<hash> → target/<profile>/russet
    let profile_dir = exe.parent().and_then(Path::parent).ok_or_else(|| io::Error::other("no target directory"))?;
    let bin = profile_dir.join(format!("russet{}", std::env::consts::EXE_SUFFIX));
    if bin.is_file() {
        Ok(bin)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} does not exist: run `cargo build -p russet-cli` first, or set RUSSET_TEST_BIN", bin.display()),
        ))
    }
}
