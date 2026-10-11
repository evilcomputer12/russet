// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The daemon's single-instance lock (`runtime/russetd.lock`, ARCHITECTURE §3.3 item 2) and the host-wide
//! live-device lock (`$XDG_RUNTIME_DIR/russet-live-device.lock`, item 7 [D 83]).
//!
//! Both are `flock`s (`LockFileEx` on Windows) held for the daemon's whole life; the kernel drops them when
//! the process dies, so a crashed daemon never leaves a stale lock. The live-device lock sits outside every
//! `RUSSET_HOME`, so two worktrees, or a release and a development daemon, never own the phone at once. Its
//! holder writes `{pid, home}` into the file for the 3019 error of the next daemon. Debug builds honour
//! `RUSSET_LIVE_DEVICE_LOCK=<path>` so tests never take the real lock; release builds ignore it.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use russet_core::error::{Error, Result, codes};
use serde::{Deserialize, Serialize};

/// The holder of the live-device lock, as recorded in the lock file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockHolder {
    pub pid: u32,
    /// The holder's `RUSSET_HOME`, or its runtime directory without one.
    pub home: String,
}

/// An exclusive file lock, released when dropped.
#[derive(Debug)]
pub struct FileLock {
    file: File,
    path: PathBuf,
}

impl FileLock {
    /// Takes the lock without waiting; `Ok(None)` when another process holds it.
    pub fn try_take(path: &Path) -> Result<Option<Self>> {
        if let Some(dir) = path.parent() {
            russet_core::fsx::ensure_private_dir(dir)?;
        }
        let mut o = OpenOptions::new();
        o.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let file = o.open(path).map_err(|e| Error::io(format!("opening {}", path.display()), e))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { file, path: path.to_path_buf() })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(Error::io(format!("locking {}", path.display()), e)),
        }
    }

    /// Replaces the file's contents (the holder record).
    pub fn write_contents(&mut self, bytes: &[u8]) -> Result<()> {
        let path = self.path.clone();
        let write = |f: &mut File| -> std::io::Result<()> {
            f.set_len(0)?;
            f.seek(SeekFrom::Start(0))?;
            f.write_all(bytes)?;
            f.sync_all()
        };
        write(&mut self.file).map_err(|e| Error::io(format!("writing {}", path.display()), e))
    }

    /// The locked file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Reads the holder record of a live-device lock file (`None` when empty or unreadable).
pub fn read_holder(path: &Path) -> Option<LockHolder> {
    let mut text = String::new();
    File::open(path).ok()?.read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

/// Where the host-wide live-device lock lives: `RUSSET_LIVE_DEVICE_LOCK` in debug builds, otherwise
/// `$XDG_RUNTIME_DIR/russet-live-device.lock` (or `/tmp/russet-<uid>/…` without it; on Windows the default
/// runtime directory until W27a's named mutex).
pub fn live_device_lock_path() -> PathBuf {
    if cfg!(debug_assertions)
        && let Some(p) = std::env::var_os("RUSSET_LIVE_DEVICE_LOCK").filter(|v| !v.is_empty())
    {
        return PathBuf::from(p);
    }
    #[cfg(unix)]
    {
        match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
            Some(dir) => dir.join("russet-live-device.lock"),
            None => PathBuf::from(format!("/tmp/russet-{}", russet_core::platform::current_uid()))
                .join("russet-live-device.lock"),
        }
    }
    #[cfg(not(unix))]
    {
        russet_core::dirs::RussetDirs::default_runtime()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("russet-live-device.lock")
    }
}

/// Takes the live-device lock and records the holder; `Err(holder)` when another daemon holds it.
pub fn take_live_device_lock(
    path: &Path,
    me: &LockHolder,
) -> Result<std::result::Result<FileLock, Option<LockHolder>>> {
    match FileLock::try_take(path)? {
        Some(mut lock) => {
            let record = serde_json::to_vec(me).map_err(|e| Error::internal(format!("encoding the holder: {e}")))?;
            lock.write_contents(&record)?;
            Ok(Ok(lock))
        }
        None => Ok(Err(read_holder(path))),
    }
}

/// The error a second daemon gets for the daemon lock.
pub fn already_running(path: &Path) -> Error {
    Error::new(
        codes::ipc::DAEMON_UNAVAILABLE,
        format!("a daemon already runs for this home ({} is locked)", path.display()),
    )
    .with_hint("Use it (`russet daemon status`), or stop it with `russet daemon stop`.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_excludes_a_second_holder_until_dropped() {
        let tmp = russet_core::fsx::TempDir::new("lock-test").expect("tempdir");
        let path = tmp.path().join("live.lock");
        let me = LockHolder { pid: 1, home: "/a".into() };
        let held = take_live_device_lock(&path, &me).expect("io").expect("free");
        let other = LockHolder { pid: 2, home: "/b".into() };
        let refused = take_live_device_lock(&path, &other).expect("io").expect_err("held");
        assert_eq!(refused, Some(me.clone()), "the second taker learns the holder");
        drop(held);
        let again = take_live_device_lock(&path, &other).expect("io").expect("free again");
        assert_eq!(read_holder(again.path()), Some(other));
    }
}
