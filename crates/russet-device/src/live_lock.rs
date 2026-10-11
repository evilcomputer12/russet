// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The host-wide live-device lock (ARCHITECTURE §3.3 item 7 [D 83]) for the lane-A tools that drive the live
//! backend without a daemon: `examples/devlab.rs` and `tests/hil.rs`.
//!
//! It is the same lock russetd's live daemon holds (W2's `daemon::lock`): an exclusive `flock` on
//! `$XDG_RUNTIME_DIR/russet-live-device.lock` (`/tmp/russet-<uid>/…` without it; `RUSSET_LIVE_DEVICE_LOCK` in debug
//! builds), whose holder writes `{pid, home}` into the file. A lab tool takes it before it constructs a
//! `LiveConnector`, so it never talks to the phone while a live daemon does, and a live daemon started meanwhile
//! answers `device.liveBackendRefused` (3019) naming the tool as the holder. The kernel drops the lock when the
//! process exits.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use russet_core::Result;
use russet_core::error::{Error, codes};
use serde::{Deserialize, Serialize};

/// The holder record in the lock file (the shape russetd writes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockHolder {
    pub pid: u32,
    /// The holder's `RUSSET_HOME`, or its runtime directory.
    pub home: String,
}

/// The held lock; dropping it releases the lock.
#[derive(Debug)]
pub struct LiveDeviceLock {
    file: File,
    path: PathBuf,
}

impl LiveDeviceLock {
    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LiveDeviceLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// The current user's id without libc: the owner of `/proc/self` (Linux).
#[cfg(unix)]
fn current_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

/// Where the lock lives: `RUSSET_LIVE_DEVICE_LOCK` in debug builds, else `$XDG_RUNTIME_DIR/russet-live-device.lock`,
/// else `/tmp/russet-<uid>/russet-live-device.lock` (the rules of russetd's `live_device_lock_path`).
pub fn lock_path() -> Result<PathBuf> {
    if cfg!(debug_assertions)
        && let Some(p) = std::env::var_os("RUSSET_LIVE_DEVICE_LOCK").filter(|v| !v.is_empty())
    {
        return Ok(PathBuf::from(p));
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return Ok(dir.join("russet-live-device.lock"));
    }
    #[cfg(unix)]
    if let Some(uid) = current_uid() {
        return Ok(PathBuf::from(format!("/tmp/russet-{uid}")).join("russet-live-device.lock"));
    }
    Err(Error::new(codes::device::LIVE_BACKEND_REFUSED, "cannot locate the live-device lock")
        .with_hint("Set XDG_RUNTIME_DIR (or RUSSET_LIVE_DEVICE_LOCK in a debug build)."))
}

/// Reads the holder record of a lock file.
pub fn read_holder(path: &Path) -> Option<LockHolder> {
    let mut text = String::new();
    File::open(path).ok()?.read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

/// Takes the lock at `path` for `me`, without waiting. `device.liveBackendRefused` (3019, with `details.holder`)
/// when a daemon or another lab tool holds it.
pub fn take_at(path: &Path, me: &LockHolder) -> Result<LiveDeviceLock> {
    let io = |what: &str, e: std::io::Error| {
        Error::new(codes::core::IO, format!("{what} {}: {e}", path.display())).caused_by(e)
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            if !dir.is_dir() {
                std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| io("creating", e))?;
            }
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(dir).map_err(|e| io("creating", e))?;
    }
    let mut o = OpenOptions::new();
    o.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut file = o.open(path).map_err(|e| io("opening", e))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            let holder = read_holder(path);
            return Err(Error::new(
                codes::device::LIVE_BACKEND_REFUSED,
                "another process holds the live-device lock (a live daemon or a lab tool)",
            )
            .with_hint("Only one process may own the phone: stop the holder (its pid is in the details) first.")
            .with_details(serde_json::json!({ "reason": "held-elsewhere", "holder": holder })));
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(io("locking", e)),
    }
    let record = serde_json::to_vec(me).map_err(|e| Error::new(codes::core::INTERNAL, format!("{e}")))?;
    let write = |f: &mut File| -> std::io::Result<()> {
        f.set_len(0)?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&record)?;
        f.sync_all()
    };
    write(&mut file).map_err(|e| io("writing", e))?;
    Ok(LiveDeviceLock { file, path: path.to_path_buf() })
}

/// Takes the host-wide lock for this process (`home`: its `RUSSET_HOME`).
pub fn take(home: &Path) -> Result<LiveDeviceLock> {
    take_at(&lock_path()?, &LockHolder { pid: std::process::id(), home: home.display().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_is_refused_with_the_first_ones_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("locks").join("live.lock");
        let me = LockHolder { pid: 1, home: "/a".into() };
        let held = take_at(&path, &me).expect("free");
        let err = take_at(&path, &LockHolder { pid: 2, home: "/b".into() }).expect_err("held");
        assert_eq!(err.code(), codes::device::LIVE_BACKEND_REFUSED);
        assert_eq!(err.code().number(), 3019);
        assert_eq!(err.to_data().details.expect("details")["holder"]["pid"], 1);
        drop(held);
        let again = take_at(&path, &LockHolder { pid: 2, home: "/b".into() }).expect("free again");
        assert_eq!(read_holder(again.path()).map(|h| h.pid), Some(2));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("lock file").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
