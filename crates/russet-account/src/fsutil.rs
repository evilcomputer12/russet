// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! File helpers: private directories, atomic 0600 writes, RFC 3339 times, and plan IDs.
//!
//! `russet_core::fsx` and `russet_core::ids` own these concerns once W2 fills them (CORE-02); this module keeps
//! W6 independent of that order and can switch to them later.

use std::path::Path;
use std::time::SystemTime;

use russet_core::Result;
use russet_core::error::{Error, codes};

/// `core.io` with the path.
pub(crate) fn io_error(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::new(codes::core::IO, format!("{what} {}: {e}", path.display())).caused_by(e)
}

/// Creates `dir` (and parents) owner-only (0700 on Unix).
pub(crate) fn private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| io_error("cannot create", dir, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| io_error("cannot chmod", dir, e))?;
    }
    Ok(())
}

/// Writes `data` to `path` atomically (temporary file, fsync, rename), mode 0600 on Unix.
pub(crate) fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    private_dir(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| io_error("cannot write", path, e))
}

/// RFC 3339 UTC with seconds (`2026-10-11T09:00:00Z`).
pub fn rfc3339(t: SystemTime) -> String {
    let dt = time::OffsetDateTime::from(t);
    dt.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

/// Parses RFC 3339 (App Store Connect dates, files).
pub fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok().map(SystemTime::from)
}

/// A prefixed ULID (`pln_01J…`, ARCHITECTURE §7.3): 48 bits of milliseconds and 80 random bits, Crockford base32.
pub(crate) fn prefixed_ulid(prefix: &str) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let ms = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let mut rand = [0u8; 10];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut rand);
    let mut value: u128 = u128::from(ms & 0xffff_ffff_ffff) << 80;
    for (i, b) in rand.iter().enumerate() {
        value |= u128::from(*b) << (72 - 8 * i);
    }
    let mut s = String::with_capacity(prefix.len() + 26);
    s.push_str(prefix);
    for i in (0..26).rev() {
        s.push(ALPHABET[((value >> (5 * i)) & 31) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn ulids_and_times() {
        let a = prefixed_ulid("pln_");
        let b = prefixed_ulid("pln_");
        assert_eq!(a.len(), 30);
        assert!(a.starts_with("pln_") && a[4..].chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
        assert_ne!(a, b);
        let t = parse_rfc3339("2027-10-10T12:00:00Z").unwrap();
        assert_eq!(rfc3339(t), "2027-10-10T12:00:00Z");
        assert!(parse_rfc3339("not a date").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn private_files() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("russet-account-fs-{}", std::process::id()));
        let p = dir.join("sub/file.toml");
        write_private(&p, b"x").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(p.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
