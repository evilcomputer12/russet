// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Filesystem helpers (core §5.1): atomic writes, the free-space guard and private directories.
//!
//! Files Russet writes under its directories are private: [`atomic_write`] and [`open_append`] create them
//! 0600 on Unix (ARCHITECTURE §14.8), and [`ensure_private_dir`] creates directories 0700 and refuses one that
//! another user owns.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, codes};

/// Writes `b` to a temporary file in the same directory, fsyncs it and renames it over `p` (mode 0600 on Unix).
pub fn atomic_write(p: &Path, b: &[u8]) -> crate::Result<()> {
    atomic_write_mode(p, b, 0o600)
}

/// [`atomic_write`] with an explicit Unix mode (ignored on Windows).
pub fn atomic_write_mode(p: &Path, b: &[u8], mode: u32) -> crate::Result<()> {
    let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = p.file_name().ok_or_else(|| Error::invalid_argument(format!("{} has no file name", p.display())))?;
    let mut suffix = [0u8; 6];
    crate::time::os_fill(&mut suffix);
    let tmp: PathBuf = dir.join(format!(
        ".{}.tmp-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        suffix.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ));
    let write = || -> std::io::Result<()> {
        let mut f = create_new(&tmp, mode)?;
        f.write_all(b)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, p)?;
        sync_dir(dir);
        Ok(())
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::io(format!("writing {}", p.display()), e)
    })
}

/// Opens `p` for appending, creating it with mode 0600 (Unix) if needed: the append-only logs (`audit.jsonl`,
/// job events).
pub fn open_append(p: &Path) -> crate::Result<File> {
    let mut o = OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(p).map_err(|e| Error::io(format!("opening {}", p.display()), e))
}

fn create_new(p: &Path, mode: u32) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    o.open(p)
}

/// Makes a rename durable (Unix); best effort.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Bytes available to the current user on `p`'s filesystem (the nearest existing ancestor of `p`).
pub fn free_space(p: &Path) -> crate::Result<u64> {
    let existing = p.ancestors().find(|a| a.exists()).unwrap_or(p);
    fs4::available_space(existing).map_err(|e| Error::io(format!("reading free space of {}", existing.display()), e))
}

/// Fails with `core.diskFull` (and a hint) unless `needed + reserve` bytes are free on `p`'s filesystem.
pub fn ensure_space(p: &Path, needed: u64, reserve: u64) -> crate::Result<()> {
    let available = free_space(p)?;
    let wanted = needed.saturating_add(reserve);
    if available >= wanted {
        return Ok(());
    }
    Err(Error::new(
        codes::core::DISK_FULL,
        format!(
            "not enough free space on {}: {} needed plus a {} reserve, {} available",
            p.display(),
            human_bytes(needed),
            human_bytes(reserve),
            human_bytes(available)
        ),
    )
    .with_hint("Free some space (`russet toolchain gc`, `russet sdk remove`, old caches) and retry.")
    .with_details(serde_json::json!({
        "path": p.display().to_string(),
        "neededBytes": needed,
        "reserveBytes": reserve,
        "availableBytes": available,
    })))
}

/// Creates `p` 0700 and checks that the current user owns it (Unix), or sets a user-only ACL (Windows).
///
/// An existing directory with group or other permission bits is tightened to 0700; one that another user owns
/// is refused (`core.io`), which also defeats a symlink planted in a shared directory such as `/tmp`.
pub fn ensure_private_dir(p: &Path) -> crate::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !p.is_dir() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(p)
                .map_err(|e| Error::io(format!("creating {}", p.display()), e))?;
        }
        let meta = std::fs::symlink_metadata(p).map_err(|e| Error::io(format!("reading {}", p.display()), e))?;
        if meta.file_type().is_symlink() {
            // A symlink is fine only if it resolves to a directory we own; check the target below.
            let target = std::fs::metadata(p).map_err(|e| Error::io(format!("reading {}", p.display()), e))?;
            return check_owned_dir(p, &target);
        }
        check_owned_dir(p, &meta)?;
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| Error::io(format!("restricting {} to 0700", p.display()), e))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // Windows: %LOCALAPPDATA% and %APPDATA% are user-only by default; explicit ACLs come with W27a.
        std::fs::create_dir_all(p).map_err(|e| Error::io(format!("creating {}", p.display()), e))
    }
}

#[cfg(unix)]
fn check_owned_dir(p: &Path, meta: &std::fs::Metadata) -> crate::Result<()> {
    use std::os::unix::fs::MetadataExt;
    if !meta.is_dir() {
        return Err(Error::new(codes::core::IO, format!("{} exists and is not a directory", p.display()))
            .with_hint("Move it out of the way; Russet needs a private directory there."));
    }
    let me = crate::platform::current_uid();
    if meta.uid() != me {
        return Err(Error::new(
            codes::core::IO,
            format!("{} is owned by uid {}, not by the current user ({me})", p.display(), meta.uid()),
        )
        .with_hint("Another user owns this directory; remove it or point RUSSET_HOME elsewhere."));
    }
    Ok(())
}

/// A private temporary directory, removed with its contents when dropped: tests (each one gets its own
/// `RUSSET_HOME`, ARCHITECTURE §3.3 item 7) and scratch work.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates `<temp>/russet-<prefix>-<random>` with mode 0700.
    pub fn new(prefix: &str) -> crate::Result<Self> {
        let mut suffix = [0u8; 8];
        crate::time::os_fill(&mut suffix);
        let name = format!("russet-{prefix}-{}", suffix.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let path = std::env::temp_dir().join(name);
        ensure_private_dir(&path)?;
        Ok(Self { path })
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A byte count for messages: `5.0 GiB`, `46.2 MiB`, `512 B`.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64;
    let mut unit = "B";
    for u in UNITS {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = u;
    }
    format!("{value:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_is_private() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("state.json");
        atomic_write(&p, b"one").expect("first write");
        atomic_write(&p, b"two").expect("second write");
        assert_eq!(std::fs::read(&p).expect("read"), b"two");
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("readdir")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files left behind");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).expect("stat").permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn ensure_space_reports_disk_full_with_details() {
        let tmp = tempfile::tempdir().expect("tempdir");
        ensure_space(tmp.path(), 1, 0).expect("one byte is free");
        let e = ensure_space(&tmp.path().join("not/yet/created"), u64::MAX / 2, u64::MAX / 2).expect_err("too much");
        assert_eq!(e.code(), codes::core::DISK_FULL);
        assert!(e.hint().is_some());
        assert!(e.details().and_then(|d| d.get("availableBytes")).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn private_dirs_are_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let d = tmp.path().join("wide");
        std::fs::create_dir(&d).expect("mkdir");
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        ensure_private_dir(&d).expect("tightens");
        assert_eq!(std::fs::metadata(&d).expect("stat").permissions().mode() & 0o777, 0o700);
        let file = tmp.path().join("file");
        std::fs::write(&file, b"x").expect("write");
        assert!(ensure_private_dir(&file).is_err(), "a file is not a directory");
    }

    #[test]
    fn bytes_read_well() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
        assert_eq!(human_bytes(1536), "1.5 KiB");
    }
}
