// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Small file helpers for session state and sockets: private (0700) directories, atomic writes, the current
//! uid and a stable hash. russet-core's `fsx` versions take over once their bodies land (W2, CORE-02).

use std::io;
use std::path::Path;

/// Creates `dir` and its parents; the leaf is made 0700 (Unix).
pub fn private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !dir.exists() {
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent)?;
            }
            match std::fs::DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Writes `bytes` to a temporary sibling and renames it over `path`.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("no parent directory"))?;
    std::fs::create_dir_all(dir)?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Unique per write, so two writers of the same file never share a temporary.
    let tmp = dir.join(format!(".{name}.{}.{n}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// The current user's uid, read from a file the process just created (std has no `getuid`).
pub fn current_uid() -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata("/proc/self").map(|m| m.uid()).ok().or_else(|| {
            let probe = std::env::temp_dir().join(format!(".russet-uid-{}", std::process::id()));
            let uid = std::fs::write(&probe, b"").and_then(|()| std::fs::metadata(&probe)).map(|m| m.uid()).ok();
            let _ = std::fs::remove_file(&probe);
            uid
        })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// 64-bit FNV-1a, for short stable names.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_dirs_and_atomic_writes() {
        let base = std::env::temp_dir().join(format!("russet-fsutil-{}", std::process::id()));
        let dir = base.join("a/b");
        private_dir(&dir).unwrap_or_else(|e| panic!("{e}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
            assert_eq!(mode, 0o700);
        }
        let f = dir.join("x.json");
        atomic_write(&f, b"{}").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(std::fs::read(&f).unwrap_or_default(), b"{}");
        let _ = std::fs::remove_dir_all(&base);
        assert_ne!(fnv1a(b"a"), fnv1a(b"b"));
        #[cfg(unix)]
        assert!(current_uid().is_some());
    }
}
