// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The autostart client (ARCHITECTURE §3.3 items 1 and 3; core §6.3).
//!
//! [`connect_or_spawn`]:
//! 1. tries the endpoint for 200 ms;
//! 2. otherwise takes `runtime/spawn.lock` (waiting at most 10 s), tries again (another client may have started
//!    the daemon meanwhile), and spawns **its own executable** as `daemon run --detach` with stdio closed;
//! 3. polls the endpoint with backoff for up to `ready_timeout`;
//! 4. compares a development client's build id, executable identity and device backend with the daemon's
//!    (ARCHITECTURE §3.3 item 3 [D 63, D 83]): on a difference it asks the daemon for
//!    `daemon.shutdown {reason: "upgrade"}`, which an idle daemon accepts, waits for it to go and starts its
//!    own build; a busy daemon gives `ipc.buildMismatch` (1107) with `restartable: false`. A daemon of another
//!    protocol major that answers `ipc.protocolMismatch` with `restartable: true` shuts itself down the same
//!    way.
//!
//! A daemon that is shutting down keeps its endpoint until its jobs have had their grace (core §6.3) and
//! answers `initialize` with `ipc.daemonUnavailable` and `details.shuttingDown`; the client waits for it to go
//! (at most [`SHUTDOWN_WAIT`]) and then starts a new one, rather than racing it for `runtime/russetd.lock`.
//!
//! `RUSSET_NO_AUTOSTART=1` turns step 2 into `ipc.daemonUnavailable`.

use std::fs::File;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result, codes};
use russet_core::platform::ExecutableId;
use russet_protocol::daemon::{DaemonShutdown, DaemonShutdownParams, ShutdownReason};
use russet_protocol::initialize::{BuildMismatchDetails, Initialize, InitializeResult, ServerInfo};

use crate::peer::{Peer, Session};
use crate::transport::Endpoint;
use crate::{ClientInfo, SpawnSpec};

/// How long the first connection attempt may take (ARCHITECTURE §3.3 item 1).
pub const FIRST_TRY: Duration = Duration::from_millis(200);

/// How long a client waits for `runtime/spawn.lock`.
pub const SPAWN_LOCK_WAIT: Duration = Duration::from_secs(10);

/// How long a client waits for a stopping daemon to go: its jobs' grace (10 s, core §6.3) plus its cleanup.
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(15);

/// Whether `e` is a stopping daemon's answer to `initialize` (`ipc.daemonUnavailable` with
/// `details.shuttingDown`).
pub fn is_shutting_down(e: &Error) -> bool {
    e.code() == codes::ipc::DAEMON_UNAVAILABLE
        && e.details().and_then(|d| d.get("shuttingDown")).and_then(serde_json::Value::as_bool) == Some(true)
}

/// What a development client requires of the daemon it talks to (ARCHITECTURE §3.3 item 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevelopmentCheck {
    /// The client's own build id.
    pub build_id: String,
    /// The identity of the client's own `current_exe()`.
    pub executable: ExecutableId,
    /// The device backend the client's environment asks for (`RUSSET_DEVICE_BACKEND`, default `live`).
    pub device_backend: String,
}

impl DevelopmentCheck {
    /// This process's check: its build id, its executable and `RUSSET_DEVICE_BACKEND`.
    pub fn for_this_process() -> Result<Self> {
        let (_, executable) = russet_core::platform::current_executable()?;
        let device_backend = std::env::var("RUSSET_DEVICE_BACKEND")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "live".to_owned());
        Ok(Self { build_id: russet_core::build_info::BUILD_ID.to_owned(), executable, device_backend })
    }

    /// The first difference from the daemon's identity, if any.
    pub fn mismatch(&self, result: &InitializeResult) -> Option<String> {
        let server = &result.server;
        let mine = russet_protocol::initialize::ExecutableId::from(self.executable);
        if server.build_id != self.build_id {
            Some(format!("the daemon is build {}, this client {}", server.build_id, self.build_id))
        } else if server.executable_id != mine {
            Some(format!("the daemon runs an older copy of {} (rebuilt since it started)", server.executable))
        } else if server.device_backend != self.device_backend {
            Some(format!(
                "the daemon uses the device backend `{}`, this client asks for `{}`",
                server.device_backend, self.device_backend
            ))
        } else {
            None
        }
    }
}

/// Connects and performs `initialize`.
pub async fn connect(endpoint: &Endpoint, client: ClientInfo) -> Result<(Peer, InitializeResult)> {
    connect_with(endpoint, client, Duration::from_secs(5)).await
}

/// [`connect`] with a connection timeout.
pub async fn connect_with(
    endpoint: &Endpoint,
    client: ClientInfo,
    timeout: Duration,
) -> Result<(Peer, InitializeResult)> {
    let peer = endpoint.open(timeout).await?;
    match initialize(&peer, &client, endpoint.token()).await {
        Ok(result) => Ok((peer, result)),
        Err(e) => {
            peer.close();
            Err(e)
        }
    }
}

/// Sends `initialize` on a client-role `peer` (for example the client half of [`crate::in_process`]) and
/// records the session.
pub async fn initialize(peer: &Peer, client: &ClientInfo, token: Option<&str>) -> Result<InitializeResult> {
    let result = peer.call::<Initialize>(client.initialize_params(token)).await?;
    peer.set_session(Session {
        conn: russet_core::rpc::ConnInfo {
            id: result.connection_id.clone(),
            kind: client.kind,
            name: client.name.clone(),
            interactive: client.capabilities.interactive,
            shared_lease: client.shared_lease.clone(),
        },
        grant: result.grant.clone(),
        capabilities: client.capabilities,
        client_version: client.version.clone(),
        client_pid: Some(client.pid),
        workspace_roots: client.workspace_roots.clone(),
    });
    Ok(result)
}

/// Connects, spawning the daemon first when needed (ARCHITECTURE §3.3 item 1).
pub async fn connect_or_spawn(
    dirs: &RussetDirs,
    client: ClientInfo,
    spawn: SpawnSpec,
) -> Result<(Peer, InitializeResult)> {
    let endpoint = Endpoint::from_env(dirs)?;
    // At most one restart of a stale daemon per call, so two clients of different builds cannot loop.
    let mut restarted = false;
    loop {
        match connect_with(&endpoint, client.clone(), FIRST_TRY).await {
            Ok((peer, result)) => {
                let Some(check) = &spawn.expect else { return Ok((peer, result)) };
                let Some(why) = check.mismatch(&result) else { return Ok((peer, result)) };
                if restarted {
                    return Err(build_mismatch(&result, check, &why, false));
                }
                tracing::info!("restarting the daemon: {why}");
                let answer = peer
                    .call::<DaemonShutdown>(DaemonShutdownParams {
                        force: false,
                        reason: Some(ShutdownReason::Upgrade),
                    })
                    .await;
                peer.close();
                match answer {
                    Ok(r) if r.accepted => {
                        wait_until_gone(&endpoint, &client, Some(&result.server), spawn.ready_timeout).await?;
                        restarted = true;
                    }
                    _ => return Err(build_mismatch(&result, check, &why, false)),
                }
            }
            Err(e) if e.code() == codes::ipc::PROTOCOL_MISMATCH && !restarted && restartable(&e) => {
                wait_until_gone(&endpoint, &client, None, spawn.ready_timeout).await?;
                restarted = true;
            }
            Err(e) if is_shutting_down(&e) => {
                tracing::info!("the daemon is shutting down: waiting for it to go before starting a new one");
                wait_until_gone(&endpoint, &client, None, SHUTDOWN_WAIT).await?;
            }
            Err(e) if e.code() == codes::ipc::DAEMON_UNAVAILABLE => {}
            Err(e) => return Err(e),
        }
        // Nothing (usable) listens: start our own daemon.
        if std::env::var_os("RUSSET_NO_AUTOSTART").is_some_and(|v| v == "1") {
            return Err(Error::new(codes::ipc::DAEMON_UNAVAILABLE, format!("no daemon listens at {endpoint}"))
                .with_hint("RUSSET_NO_AUTOSTART=1 is set: start one with `russet daemon start`."));
        }
        russet_core::fsx::ensure_private_dir(&dirs.runtime)?;
        let lock = take_spawn_lock(&dirs.spawn_lock()).await?;
        // Another client may have started it while we waited for the lock.
        match connect_with(&endpoint, client.clone(), FIRST_TRY).await {
            Ok((peer, result)) => {
                drop(lock);
                let mismatch = spawn.expect.as_ref().and_then(|c| c.mismatch(&result).map(|why| (c, why)));
                match mismatch {
                    None => return Ok((peer, result)),
                    // A daemon of another build appeared meanwhile: the top of the loop restarts it.
                    Some(_) if !restarted => {
                        peer.close();
                        continue;
                    }
                    Some((check, why)) => {
                        peer.close();
                        return Err(build_mismatch(&result, check, &why, false));
                    }
                }
            }
            Err(e) if is_shutting_down(&e) => {
                if let Err(e) = wait_until_gone(&endpoint, &client, None, SHUTDOWN_WAIT).await {
                    drop(lock);
                    return Err(e);
                }
            }
            Err(e) if e.code() == codes::ipc::DAEMON_UNAVAILABLE => {}
            Err(e) => return Err(e),
        }
        spawn_daemon(&spawn)?;
        let deadline = Instant::now() + spawn.ready_timeout;
        let mut delay = Duration::from_millis(20);
        let connected = loop {
            match connect_with(&endpoint, client.clone(), FIRST_TRY).await {
                Ok(ok) => break Ok(ok),
                Err(e) if e.code() == codes::ipc::DAEMON_UNAVAILABLE && Instant::now() < deadline => {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_millis(250));
                }
                Err(e) if e.code() == codes::ipc::DAEMON_UNAVAILABLE => {
                    break Err(Error::new(
                        codes::ipc::DAEMON_UNAVAILABLE,
                        format!("the daemon did not start within {:?}", spawn.ready_timeout),
                    )
                    .with_hint("Check `russet daemon logs`, or run `russet daemon run` in a terminal to see why.")
                    .caused_by(e));
                }
                Err(e) => break Err(e),
            }
        };
        drop(lock);
        let (peer, result) = connected?;
        if let Some(check) = &spawn.expect
            && let Some(why) = check.mismatch(&result)
        {
            peer.close();
            return Err(build_mismatch(&result, check, &why, false));
        }
        return Ok((peer, result));
    }
}

fn restartable(e: &Error) -> bool {
    e.details().and_then(|d| d.get("restartable")).and_then(serde_json::Value::as_bool).unwrap_or(false)
}

fn build_mismatch(result: &InitializeResult, check: &DevelopmentCheck, why: &str, restartable: bool) -> Error {
    let details = BuildMismatchDetails {
        server_build_id: result.server.build_id.clone(),
        client_build_id: check.build_id.clone(),
        server_executable: result.server.executable.clone(),
        restartable,
    };
    Error::new(codes::ipc::BUILD_MISMATCH, format!("the running daemon is another build: {why}"))
        .with_hint("It is busy; let its jobs finish, then run `russet daemon restart --force`.")
        .with_details(serde_json::to_value(details).unwrap_or_default())
}

/// Waits until the daemon at `endpoint` has gone: nothing listens, or a daemon other than `old` answers
/// `initialize` (another client started the next one meanwhile). While it stops, the old daemon answers
/// `initialize` with `shuttingDown`, with `ipc.protocolMismatch` (another protocol major) or, briefly after
/// accepting `daemon.shutdown`, as `old` itself.
async fn wait_until_gone(
    endpoint: &Endpoint,
    client: &ClientInfo,
    old: Option<&ServerInfo>,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let still_there = match connect_with(endpoint, client.clone(), FIRST_TRY).await {
            Err(e) if is_shutting_down(&e) => true,
            Err(e) if matches!(e.code(), codes::ipc::PROTOCOL_MISMATCH | codes::ipc::CONNECTION_CLOSED) => true,
            Err(e) if e.code() == codes::ipc::DAEMON_UNAVAILABLE => return Ok(()),
            Err(e) => return Err(e),
            Ok((peer, result)) => {
                peer.close();
                old.is_some_and(|old| result.server.pid == old.pid && result.server.started_at == old.started_at)
            }
        };
        if !still_there {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::new(codes::ipc::DAEMON_UNAVAILABLE, "the old daemon did not exit in time")
                .with_hint("Stop it with `russet daemon stop --force`."));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Holds `runtime/spawn.lock` until dropped.
struct SpawnLock(File);

impl Drop for SpawnLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

async fn take_spawn_lock(path: &Path) -> Result<SpawnLock> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|e| Error::io(format!("opening {}", path.display()), e))?;
    let deadline = Instant::now() + SPAWN_LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(SpawnLock(file)),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::new(codes::ipc::DAEMON_STARTING, "another client is still starting the daemon"));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(Error::io(format!("locking {}", path.display()), e)),
        }
    }
}

/// Starts the daemon detached: stdio closed, its own session (the daemon calls `setsid` for `--detach`) on
/// Unix, `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW` on
/// Windows (core §6.3).
fn spawn_daemon(spec: &SpawnSpec) -> Result<()> {
    let mut cmd = std::process::Command::new(&spec.executable);
    cmd.args(&spec.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW);
    }
    let child = cmd.spawn().map_err(|e| {
        Error::new(
            codes::ipc::DAEMON_UNAVAILABLE,
            format!("cannot start the daemon ({}): {e}", spec.executable.display()),
        )
        .caused_by(e)
    })?;
    // The daemon outlives this process; reap it in the background if it exits first (a refused start).
    std::thread::Builder::new()
        .name("russet-daemon-reaper".to_owned())
        .spawn(move || {
            let mut child = child;
            let _ = child.wait();
        })
        .map_err(|e| Error::io("starting the reaper thread", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopping_daemon_is_told_apart_from_a_missing_one() {
        let stopping = Error::new(codes::ipc::DAEMON_UNAVAILABLE, "the daemon is shutting down")
            .with_details(serde_json::json!({"shuttingDown": true, "graceMs": 10_000}));
        assert!(is_shutting_down(&stopping));
        assert!(!is_shutting_down(&Error::new(codes::ipc::DAEMON_UNAVAILABLE, "nothing listens")));
        let other =
            Error::new(codes::ipc::PROTOCOL_MISMATCH, "x").with_details(serde_json::json!({"shuttingDown": true}));
        assert!(!is_shutting_down(&other));
    }
}
