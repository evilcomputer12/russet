// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Deterministic IPA writer and reader (ARCHITECTURE §5.3 `russet-ipa`, build §4.8) [D 19].
//!
//! - **Layout:** `Payload/<App>.app/…`; store IPAs may add `SwiftSupport/iphoneos/libswift*.dylib`.
//! - **Determinism:** every entry name is sorted by bytes, directories get explicit entries, every timestamp is
//!   the same fixed time ([`IpaOptions::epoch`], from `SOURCE_DATE_EPOCH` by default, clamped to the DOS range),
//!   and sizes go in the local headers (no data descriptors), so two runs over the same bundle give the same
//!   bytes on Linux and Windows.
//! - **Modes:** directories `040755`; Mach-O files (thin or fat, detected by magic, so it works on Windows, which
//!   has no exec bit) `100755`; other files `100644`; symlinks `120777` with the target as data.
//! - **Compression:** [`IpaMode::Dev`] stores entries for fast installs; [`IpaMode::Store`] deflates at level 6.
//!   Zip64 is used for files of 4 GiB or more.
//!
//! The `zip` and `unzip` binaries are never invoked. Layer F.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipArchive, ZipWriter};

/// Errors of this crate.
#[derive(Debug, thiserror::Error)]
pub enum IpaError {
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("zip: {0}")]
    Zip(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<zip::result::ZipError> for IpaError {
    fn from(e: zip::result::ZipError) -> Self {
        match e {
            zip::result::ZipError::Io(io) => IpaError::Io(io),
            other => IpaError::Zip(other.to_string()),
        }
    }
}

/// `Result` with [`IpaError`].
pub type Result<T, E = IpaError> = std::result::Result<T, E>;

/// Compression mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpaMode {
    /// Stored entries, for fast development installs.
    Dev,
    /// Deflate level 6, for App Store uploads.
    Store,
}

/// Writer options.
#[derive(Debug, Clone)]
pub struct IpaOptions {
    pub mode: IpaMode,
    /// Swift runtime libraries to copy into `SwiftSupport/iphoneos/` (store IPAs), unmodified.
    pub swift_support: Option<Vec<Utf8PathBuf>>,
    /// The fixed modification time of every entry (`SOURCE_DATE_EPOCH`).
    pub epoch: SystemTime,
}

impl IpaOptions {
    /// Options for `mode` with the default epoch ([`default_epoch`]) and no `SwiftSupport/`.
    pub fn new(mode: IpaMode) -> Self {
        Self { mode, swift_support: None, epoch: default_epoch() }
    }
}

/// `SOURCE_DATE_EPOCH` when it is set to a number of seconds, else 2001-01-01T00:00:00Z (Apple's reference
/// date; build §4.6).
pub fn default_epoch() -> SystemTime {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map_or(UNIX_EPOCH + Duration::from_secs(978_307_200), |s| UNIX_EPOCH + Duration::from_secs(s))
}

/// What [`write()`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpaReport {
    pub path: Utf8PathBuf,
    /// Number of zip entries, directories included.
    pub entries: usize,
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the whole IPA.
    pub sha256: String,
}

/// How an entry is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Stored,
    Deflated,
    /// Any other zip method number.
    Other(u16),
}

/// One entry of an IPA listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// The full Unix mode, file type included (`0o100644`, `0o040755`, `0o120777`, …).
    pub unix_mode: u32,
    /// Uncompressed size.
    pub size: u64,
    pub symlink_target: Option<String>,
    pub compression: Compression,
}

impl Entry {
    /// Whether this is a directory entry.
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/') || self.unix_mode & 0o170000 == 0o040000
    }
}

/// Whether `head` (the first bytes of a file) starts a Mach-O image: thin (32- or 64-bit, either byte order) or
/// fat (`0xCAFEBABE`/`0xCAFEBABF` with a plausible architecture count, which tells it from a Java class file).
pub fn is_macho(head: &[u8]) -> bool {
    let Some(magic) = head.get(..4) else { return false };
    let thin = [[0xCF, 0xFA, 0xED, 0xFE], [0xCE, 0xFA, 0xED, 0xFE], [0xFE, 0xED, 0xFA, 0xCF], [0xFE, 0xED, 0xFA, 0xCE]];
    if thin.iter().any(|m| m == magic) {
        return true;
    }
    if magic == [0xCA, 0xFE, 0xBA, 0xBE] || magic == [0xCA, 0xFE, 0xBA, 0xBF] {
        let count = head.get(4..8).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        return count.is_some_and(|n| n > 0 && n < 32);
    }
    false
}

/// One planned entry.
enum Item {
    Dir,
    File { source: Utf8PathBuf, size: u64, executable: bool },
    Symlink { target: String },
}

fn head_of(path: &Utf8Path) -> Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let mut buf = [0u8; 8];
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(buf[..n].to_vec())
}

fn plan_file(source: &Utf8Path) -> Result<Item> {
    let meta = std::fs::metadata(source)?;
    Ok(Item::File { source: source.to_owned(), size: meta.len(), executable: is_macho(&head_of(source)?) })
}

/// Adds `dir` (named `name`, ending in `/`) and everything below it.
fn plan_tree(dir: &Utf8Path, name: &str, out: &mut Vec<(String, Item)>) -> Result<()> {
    out.push((name.to_owned(), Item::Dir));
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|p| IpaError::Invalid(format!("non-UTF-8 path {}", p.display())))?;
        let file_name = path.file_name().unwrap_or_default().to_owned();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            let target = std::fs::read_link(&path)?;
            let target = target
                .to_str()
                .ok_or_else(|| IpaError::Invalid(format!("non-UTF-8 symlink target in {path}")))?
                .replace('\\', "/");
            out.push((format!("{name}{file_name}"), Item::Symlink { target }));
        } else if kind.is_dir() {
            plan_tree(&path, &format!("{name}{file_name}/"), out)?;
        } else {
            out.push((format!("{name}{file_name}"), plan_file(&path)?));
        }
    }
    Ok(())
}

/// Converts `t` to a DOS timestamp in UTC, clamped to 1980-01-01 … 2107-12-31 (2-second resolution).
fn dos_time(t: SystemTime) -> DateTime {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days since 1970-01-01 to a civil date (proleptic Gregorian).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = yoe + era * 400 + i64::from(month <= 2);
    let (hour, minute, second) = ((rem / 3_600) as u8, ((rem / 60) % 60) as u8, (rem % 60) as u8);
    let minimum = DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).unwrap_or_default();
    if year < 1980 {
        return minimum;
    }
    if year > 2107 {
        return DateTime::from_date_and_time(2107, 12, 31, 23, 59, 58).unwrap_or(minimum);
    }
    DateTime::from_date_and_time(year as u16, month, day, hour, minute, second).unwrap_or(minimum)
}

/// Packs `app` (a `.app` directory) into the IPA `out`, replacing it atomically.
pub fn write(app: &Utf8Path, out: &Utf8Path, o: IpaOptions) -> Result<IpaReport> {
    let app_name = app
        .file_name()
        .filter(|n| n.ends_with(".app") && n.len() > 4)
        .ok_or_else(|| IpaError::Invalid(format!("{app} is not a .app bundle")))?;
    if !std::fs::metadata(app)?.is_dir() {
        return Err(IpaError::Invalid(format!("{app} is not a directory")));
    }
    let mut items: Vec<(String, Item)> = vec![("Payload/".to_owned(), Item::Dir)];
    plan_tree(app, &format!("Payload/{app_name}/"), &mut items)?;
    if let Some(libs) = &o.swift_support {
        items.push(("SwiftSupport/".to_owned(), Item::Dir));
        items.push(("SwiftSupport/iphoneos/".to_owned(), Item::Dir));
        for lib in libs {
            let name = lib.file_name().ok_or_else(|| IpaError::Invalid(format!("{lib} has no file name")))?;
            items.push((format!("SwiftSupport/iphoneos/{name}"), plan_file(lib)?));
        }
    }
    items.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    if let Some(w) = items.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(IpaError::Invalid(format!("duplicate entry {}", w[0].0)));
    }

    let parent = out.parent().filter(|p| !p.as_str().is_empty()).unwrap_or(Utf8Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{}.tmp-{}", out.file_name().unwrap_or("out.ipa"), std::process::id()));
    let result = write_items(&tmp, &items, &o);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, out)?;

    let (bytes, sha256) = sha256_of(out)?;
    Ok(IpaReport { path: out.to_owned(), entries: items.len(), bytes, sha256 })
}

/// The size and lowercase hex SHA-256 of a file.
fn sha256_of(path: &Utf8Path) -> Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut reader = File::open(path)?;
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((total, hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()))
}

fn write_items(tmp: &Utf8Path, items: &[(String, Item)], o: &IpaOptions) -> Result<()> {
    let file = File::create(tmp)?;
    let mut zip = ZipWriter::new(BufWriter::new(file));
    let base = SimpleFileOptions::default().system(System::Unix).last_modified_time(dos_time(o.epoch));
    let (method, level) = match o.mode {
        IpaMode::Dev => (CompressionMethod::Stored, None),
        IpaMode::Store => (CompressionMethod::Deflated, Some(6)),
    };
    for (name, item) in items {
        match item {
            Item::Dir => {
                let opts = base.compression_method(CompressionMethod::Stored).unix_permissions(0o755);
                zip.add_directory(name.as_str(), opts)?;
            }
            Item::Symlink { target } => {
                zip.add_symlink(name.as_str(), target.as_str(), base.unix_permissions(0o777))?;
            }
            Item::File { source, size, executable } => {
                let mode = if *executable { 0o755 } else { 0o644 };
                let opts = base
                    .compression_method(if *size == 0 { CompressionMethod::Stored } else { method })
                    .compression_level(if *size == 0 { None } else { level })
                    .unix_permissions(mode)
                    .large_file(*size >= u64::from(u32::MAX));
                zip.start_file(name.as_str(), opts)?;
                let mut src = BufReader::new(File::open(source)?);
                let copied = std::io::copy(&mut src, &mut zip)?;
                if copied != *size {
                    return Err(IpaError::Invalid(format!("{source} changed while it was being packed")));
                }
            }
        }
    }
    let mut inner = zip.finish()?;
    inner.flush()?;
    inner.into_inner().map_err(|e| IpaError::Io(e.into_error()))?.sync_all()?;
    Ok(())
}

fn open(ipa: &Utf8Path) -> Result<ZipArchive<BufReader<File>>> {
    Ok(ZipArchive::new(BufReader::new(File::open(ipa)?))?)
}

/// Lists the entries of an IPA, in archive order.
pub fn read_listing(ipa: &Utf8Path) -> Result<Vec<Entry>> {
    let mut archive = open(ipa)?;
    let mut entries = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        let unix_mode = f.unix_mode().unwrap_or(if f.is_dir() { 0o040755 } else { 0o100644 });
        let compression = match f.compression() {
            CompressionMethod::Stored => Compression::Stored,
            CompressionMethod::Deflated => Compression::Deflated,
            #[allow(deprecated)]
            other => Compression::Other(other.to_u16()),
        };
        let symlink_target = if unix_mode & 0o170000 == 0o120000 {
            let mut target = String::new();
            f.read_to_string(&mut target)?;
            Some(target)
        } else {
            None
        };
        entries.push(Entry { name: f.name()?.into_owned(), unix_mode, size: f.size(), symlink_target, compression });
    }
    Ok(entries)
}

/// Reads one entry's contents (for example `Payload/App.app/Info.plist`).
pub fn read_entry(ipa: &Utf8Path, name: &str) -> Result<Vec<u8>> {
    let mut archive = open(ipa)?;
    let mut f = archive.by_name(name).map_err(|e| match e {
        zip::result::ZipError::FileNotFound => IpaError::Invalid(format!("{ipa} has no entry {name}")),
        other => other.into(),
    })?;
    let mut buf = Vec::with_capacity(usize::try_from(f.size()).unwrap_or(0));
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// The `Payload/<App>.app/` directory of a listing, if there is exactly one.
pub fn app_dir(listing: &[Entry]) -> Option<String> {
    let mut apps = listing.iter().filter_map(|e| {
        let rest = e.name.strip_prefix("Payload/")?;
        let (first, _) = rest.split_once('/')?;
        first.ends_with(".app").then(|| format!("Payload/{first}/"))
    });
    let first = apps.next()?;
    apps.all(|a| a == first).then_some(first)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn macho_magic_is_recognized() {
        assert!(is_macho(&[0xCF, 0xFA, 0xED, 0xFE, 0x0C, 0, 0, 1]));
        assert!(is_macho(&[0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 2]));
        // A Java class file shares the fat magic but has a version where the architecture count would be.
        assert!(!is_macho(&[0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 0x34]));
        assert!(!is_macho(b"#!/bin/sh"));
        assert!(!is_macho(&[0xCF]));
    }

    #[test]
    fn dos_times_are_utc_and_clamped() {
        let t = UNIX_EPOCH + Duration::from_secs(978_307_200); // 2001-01-01T00:00:00Z
        let d = dos_time(t);
        assert_eq!((d.year(), d.month(), d.day(), d.hour(), d.minute(), d.second()), (2001, 1, 1, 0, 0, 0));
        let d = dos_time(UNIX_EPOCH + Duration::from_secs(1_791_700_001)); // 2026-10-11T…
        assert_eq!((d.year(), d.month(), d.day()), (2026, 10, 11));
        assert_eq!(dos_time(UNIX_EPOCH).year(), 1980);
    }

    #[test]
    fn app_dir_finds_the_single_bundle() {
        let e = |n: &str| Entry {
            name: n.into(),
            unix_mode: 0o100644,
            size: 0,
            symlink_target: None,
            compression: Compression::Stored,
        };
        assert_eq!(
            app_dir(&[e("Payload/"), e("Payload/A.app/"), e("Payload/A.app/x")]).as_deref(),
            Some("Payload/A.app/")
        );
        assert_eq!(app_dir(&[e("Payload/A.app/x"), e("Payload/B.app/y")]), None);
    }
}
