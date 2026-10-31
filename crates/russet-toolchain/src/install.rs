// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The installer (CORE-11 subset; core §10.5): pinned, verified downloads of `python-shared` and `darwin-tools`.
//!
//! 1. Free space: the download and its extraction (size × 2.2) must leave `storage.min-free-space` free.
//! 2. Download to `cache/downloads/<sha256>.part`, resuming with an HTTP `Range` request, then check the size and
//!    the SHA-256 the signed pin manifest names.
//! 3. Extract into `data/toolchains/.staging-<rand>/` (no absolute paths, no `..`, no links that leave the
//!    tree), then rename atomically to `data/toolchains/<component>/<version>/` with a `.russet-install.json`
//!    marker. Selection is `data/toolchains/selected.json`; there are no symlinks, so Windows works the same.
//! 4. Delete the download.
//!
//! `RUSSET_OFFLINE=1` refuses any non-loopback URL (tests).

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Component as PathComponent, Path, PathBuf};
use std::time::Duration;

use russet_core::error::{Error, Result, codes};
use russet_core::progress::{LogStream, ProgressSink};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio_util::sync::CancellationToken;

use crate::pins::{Artifact, ComponentPin};

/// The marker of a finished install.
pub const MARKER: &str = ".russet-install.json";
/// The selection file.
pub const SELECTED: &str = "selected.json";

/// `data/toolchains`.
pub fn toolchains_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("toolchains")
}

/// `.russet-install.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallMarker {
    pub component: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub installed_at: String,
}

/// `selected.json`: component → selected version.
pub fn read_selection(data_dir: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(toolchains_dir(data_dir).join(SELECTED))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_selection(data_dir: &Path, sel: &BTreeMap<String, String>) -> Result<()> {
    let text = serde_json::to_vec_pretty(sel).map_err(|e| {
        Error::new(codes::core::INTERNAL, format!("cannot encode selected.json: {e}")).with_hint("Report this bug.")
    })?;
    let path = toolchains_dir(data_dir).join(SELECTED);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| io_err("write", &path, e))
}

/// Installed versions of a managed component, the selected one first, then newest first.
pub fn installed(data_dir: &Path, component: &str) -> Vec<(String, PathBuf)> {
    let dir = toolchains_dir(data_dir).join(component);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join(MARKER).is_file())
        .filter_map(|p| Some((p.file_name()?.to_str()?.to_owned(), p)))
        .collect();
    let selected = read_selection(data_dir).get(component).cloned();
    out.sort_by(|(a, _), (b, _)| {
        let sel = (Some(b) == selected.as_ref()).cmp(&(Some(a) == selected.as_ref()));
        let va = a.parse::<crate::version::Version>().ok();
        let vb = b.parse::<crate::version::Version>().ok();
        sel.then_with(|| vb.cmp(&va))
    });
    out
}

fn io_err(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::new(codes::core::IO, format!("cannot {what} {}: {e}", path.display()))
        .with_hint("Check the permissions and free space of Russet's data and cache directories.")
        .caused_by(e)
}

fn download_failed(msg: impl Into<String>) -> Error {
    Error::new(codes::toolchain::DOWNLOAD_FAILED, msg)
        .with_hint("Check the network connection and retry; an interrupted download resumes where it stopped.")
        .retryable(true)
}

fn verify_failed(msg: impl Into<String>) -> Error {
    Error::new(codes::toolchain::VERIFY_FAILED, msg).with_hint(
        "The download does not match the signed pin manifest; it was deleted. Retry, and report it if it persists.",
    )
}

/// Fails with `core.diskFull` unless `needed` bytes plus `reserve` are free on `dir`'s filesystem.
pub fn ensure_space(dir: &Path, needed: u64, reserve: u64) -> Result<()> {
    let free = fs4::available_space(dir).map_err(|e| io_err("measure free space on", dir, e))?;
    if free < needed.saturating_add(reserve) {
        return Err(Error::new(
            codes::core::DISK_FULL,
            format!(
                "not enough free space on {}: {} MiB free, {} MiB needed plus the {} MiB reserve",
                dir.display(),
                free >> 20,
                needed >> 20,
                reserve >> 20
            ),
        )
        .with_hint("Free some space (`russet toolchain gc`), or lower `storage.min-free-space` in config.toml.")
        .with_details(serde_json::json!({ "freeBytes": free, "neededBytes": needed, "reserveBytes": reserve })));
    }
    Ok(())
}

/// Whether `url` names a loopback host (`127.0.0.1`, `localhost`, `[::1]`), parsed properly so userinfo
/// (`http://127.0.0.1:x@example.com/`) cannot pass as one.
pub(crate) fn is_loopback_url(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|h| matches!(h.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1"))
}

/// What one install needs besides the pin.
pub(crate) struct InstallJob<'a> {
    pub data_dir: &'a Path,
    pub cache_dir: &'a Path,
    pub min_free_space: u64,
    pub offline: bool,
    pub progress: &'a dyn ProgressSink,
    pub cancel: &'a CancellationToken,
}

impl InstallJob<'_> {
    /// Installs `pin`'s artifact for `platform`; returns the install directory. Idempotent.
    pub async fn run(&self, pin: &ComponentPin, platform: &str) -> Result<PathBuf> {
        let artifact = pin.artifacts.get(platform).ok_or_else(|| {
            Error::new(
                codes::core::UNSUPPORTED_PLATFORM,
                format!("{} {} has no build for {platform}", pin.name, pin.version),
            )
            .with_hint("The pin manifest lists the supported platforms.")
        })?;
        let target = toolchains_dir(self.data_dir).join(&pin.name).join(&pin.version);
        if let Some(existing) = read_marker(&target)
            && existing.sha256 == artifact.sha256
        {
            self.select(&pin.name, &pin.version)?;
            return Ok(target);
        }
        if self.offline && !is_loopback_url(&artifact.url) {
            return Err(download_failed(format!("RUSSET_OFFLINE is set; refusing to download {}", artifact.url))
                .with_hint("Unset RUSSET_OFFLINE to allow downloads."));
        }
        let tc_dir = toolchains_dir(self.data_dir);
        let downloads = self.cache_dir.join("downloads");
        for d in [&tc_dir, &downloads] {
            std::fs::create_dir_all(d).map_err(|e| io_err("create", d, e))?;
        }
        // The archive plus its extraction, with slack (core §10.5).
        let needed = artifact.size.saturating_mul(22) / 10;
        ensure_space(&tc_dir, needed, self.min_free_space)?;

        let title = format!("Installing {} {}", pin.name, pin.version);
        self.progress.begin(&title, true);
        let part = downloads.join(format!("{}.part", artifact.sha256));
        let result = self.download_verify_extract(pin, artifact, &part, &target).await;
        match &result {
            Ok(_) => self.progress.end(Some("installed")),
            Err(e) => self.progress.end(Some(e.message())),
        }
        result
    }

    async fn download_verify_extract(
        &self,
        pin: &ComponentPin,
        artifact: &Artifact,
        part: &Path,
        target: &Path,
    ) -> Result<PathBuf> {
        download(&artifact.url, part, artifact.size, self.progress, self.cancel).await?;
        let digest = sha256_file(part.to_path_buf()).await?;
        if digest != artifact.sha256 {
            let _ = std::fs::remove_file(part);
            return Err(verify_failed(format!(
                "{} {}: SHA-256 {digest} does not match the pinned {}",
                pin.name, pin.version, artifact.sha256
            )));
        }
        if self.cancel.is_cancelled() {
            return Err(cancelled());
        }
        self.progress.report(Some("extracting"), Some(95));
        let staging = toolchains_dir(self.data_dir).join(format!(".staging-{}", random_suffix()));
        let (part_owned, staging_owned) = (part.to_path_buf(), staging.clone());
        let strip = archive_root(&pin.name);
        let extracted = tokio::task::spawn_blocking(move || extract_tar_gz(&part_owned, &staging_owned, strip))
            .await
            .map_err(|e| {
            Error::new(codes::core::INTERNAL, format!("the extraction task failed: {e}")).with_hint("Report this bug.")
        })?;
        let root = match extracted {
            Ok(root) => root,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        let marker = InstallMarker {
            component: pin.name.clone(),
            version: pin.version.clone(),
            url: artifact.url.clone(),
            sha256: artifact.sha256.clone(),
            size: artifact.size,
            installed_at: now_rfc3339(),
        };
        let marker_text = serde_json::to_vec_pretty(&marker).map_err(|e| {
            Error::new(codes::core::INTERNAL, format!("cannot encode the marker: {e}")).with_hint("Report this bug.")
        })?;
        std::fs::write(root.join(MARKER), marker_text).map_err(|e| io_err("write", &root.join(MARKER), e))?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_err("create", parent, e))?;
        }
        if target.exists() {
            std::fs::remove_dir_all(target).map_err(|e| io_err("replace", target, e))?;
        }
        std::fs::rename(&root, target).map_err(|e| io_err("move into place", target, e))?;
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_file(part);
        self.select(&pin.name, &pin.version)?;
        Ok(target.to_path_buf())
    }

    fn select(&self, component: &str, version: &str) -> Result<()> {
        let mut sel = read_selection(self.data_dir);
        if sel.get(component).map(String::as_str) != Some(version) {
            sel.insert(component.to_owned(), version.to_owned());
            write_selection(self.data_dir, &sel)?;
        }
        Ok(())
    }
}

/// Removes an installed version; returns the bytes freed (`core.notFound` when it is not installed).
pub fn uninstall(data_dir: &Path, component: &str, version: &str) -> Result<u64> {
    let dir = toolchains_dir(data_dir).join(component).join(version);
    if !dir.join(MARKER).is_file() {
        return Err(Error::new(codes::core::NOT_FOUND, format!("{component} {version} is not installed"))
            .with_hint("`russet toolchain list` shows what is installed."));
    }
    let freed = dir_size(&dir);
    std::fs::remove_dir_all(&dir).map_err(|e| io_err("remove", &dir, e))?;
    let mut sel = read_selection(data_dir);
    if sel.get(component).map(String::as_str) == Some(version) {
        sel.remove(component);
        write_selection(data_dir, &sel)?;
    }
    Ok(freed)
}

/// Every installed version that is not selected, as `(component, version, dir, bytes)`.
pub fn unselected(data_dir: &Path) -> Vec<(String, String, PathBuf, u64)> {
    let sel = read_selection(data_dir);
    let Ok(components) = std::fs::read_dir(toolchains_dir(data_dir)) else { return Vec::new() };
    let mut out = Vec::new();
    for c in components.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()) {
        let Some(name) = c.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        if name.starts_with('.') {
            continue;
        }
        for (version, dir) in installed(data_dir, &name) {
            if sel.get(&name) != Some(&version) {
                let size = dir_size(&dir);
                out.push((name.clone(), version, dir, size));
            }
        }
    }
    out
}

/// Leftover staging directories of interrupted installs.
pub fn stale_staging(data_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(toolchains_dir(data_dir)) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(".staging-")))
        .collect()
}

/// The size of a directory tree in bytes (symlinks are not followed).
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .filter_map(Result::ok)
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

fn read_marker(dir: &Path) -> Option<InstallMarker> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(MARKER)).ok()?).ok()
}

fn cancelled() -> Error {
    Error::new(codes::core::CANCELLED, "the install was cancelled")
        .with_hint("Run it again; the download resumes where it stopped.")
}

fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = secs / 86_400;
    let (h, m, s) = ((secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Days since 1970-01-01 → (year, month, day) (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn random_suffix() -> String {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    format!("{}-{nanos:08x}", std::process::id())
}

/// The HTTP client: rustls with the ring provider (ARCHITECTURE §16.6) and conservative timeouts.
fn client() -> Result<reqwest::Client> {
    // One provider per process; installing twice is harmless.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("russet/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| download_failed(format!("cannot create the HTTP client: {e}")))
}

/// Downloads `url` into `part`, resuming a previous partial download, until it holds `size` bytes.
pub(crate) async fn download(
    url: &str,
    part: &Path,
    size: u64,
    progress: &dyn ProgressSink,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut have = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
    if have > size {
        tokio::fs::remove_file(part).await.map_err(|e| io_err("remove", part, e))?;
        have = 0;
    }
    if have == size {
        return Ok(());
    }
    let client = client()?;
    let mut req = client.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        progress.log(LogStream::Info, &format!("resuming the download at byte {have}"));
    }
    let mut resp = tokio::select! {
        r = req.send() => r.map_err(|e| download_failed(format!("GET {url}: {e}")))?,
        () = cancel.cancelled() => return Err(cancelled()),
    };
    let status = resp.status();
    let append = match status.as_u16() {
        206 if have > 0 => true,
        200 => false,
        416 => {
            let _ = tokio::fs::remove_file(part).await;
            return Err(download_failed(format!("GET {url}: the server refused to resume; retry")));
        }
        _ => return Err(download_failed(format!("GET {url}: HTTP {status}"))),
    };
    if !append {
        have = 0;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(part)
        .await
        .map_err(|e| io_err("open", part, e))?;
    let mut last_pct = u8::MAX;
    loop {
        let chunk = tokio::select! {
            c = resp.chunk() => c.map_err(|e| download_failed(format!("GET {url}: {e}")))?,
            () = cancel.cancelled() => {
                let _ = file.flush().await;
                return Err(cancelled());
            }
        };
        let Some(bytes) = chunk else { break };
        file.write_all(&bytes).await.map_err(|e| io_err("write", part, e))?;
        have += bytes.len() as u64;
        if have > size {
            drop(file);
            let _ = tokio::fs::remove_file(part).await;
            return Err(verify_failed(format!("{url} is larger than the pinned {size} bytes")));
        }
        let pct = u8::try_from(have.saturating_mul(90) / size.max(1)).unwrap_or(90);
        if pct != last_pct {
            last_pct = pct;
            progress.report(Some("downloading"), Some(pct));
        }
    }
    file.flush().await.map_err(|e| io_err("write", part, e))?;
    file.sync_all().await.map_err(|e| io_err("sync", part, e))?;
    if have != size {
        return Err(download_failed(format!(
            "{url}: the connection ended after {have} of {size} bytes; retry to resume"
        )));
    }
    Ok(())
}

/// SHA-256 of a file, on a blocking thread.
async fn sha256_file(path: PathBuf) -> Result<String> {
    tokio::task::spawn_blocking(move || -> Result<String> {
        let mut f = std::fs::File::open(&path).map_err(|e| io_err("open", &path, e))?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = f.read(&mut buf).map_err(|e| io_err("read", &path, e))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    })
    .await
    .map_err(|e| {
        Error::new(codes::core::INTERNAL, format!("the hashing task failed: {e}")).with_hint("Report this bug.")
    })?
}

/// Whether a relative path stays inside its root (no absolute paths, no `..` above the root).
fn stays_inside(p: &Path) -> bool {
    let mut depth: i32 = 0;
    for c in p.components() {
        match c {
            PathComponent::Normal(_) => depth += 1,
            PathComponent::CurDir => {}
            PathComponent::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            PathComponent::RootDir | PathComponent::Prefix(_) => return false,
        }
    }
    true
}

/// The directory a component's archive wraps its files in: python-build-standalone's `python/`; darwin-tools has
/// none (`bin/` at the top).
pub fn archive_root(component: &str) -> Option<&'static str> {
    match component {
        "python-shared" => Some("python"),
        _ => None,
    }
}

/// Extracts a `.tar.gz` into `staging` and returns the install root: `staging/<strip>` when the archive wraps its
/// files in that directory, else `staging`.
pub(crate) fn extract_tar_gz(archive: &Path, staging: &Path, strip: Option<&str>) -> Result<PathBuf> {
    let invalid = |m: String| verify_failed(format!("{}: {m}", archive.display()));
    std::fs::create_dir_all(staging).map_err(|e| io_err("create", staging, e))?;
    let file = std::fs::File::open(archive).map_err(|e| io_err("open", archive, e))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(file)));
    tar.set_preserve_permissions(false);
    tar.set_unpack_xattrs(false);
    tar.set_overwrite(true);
    tar.set_preserve_mtime(true);
    let entries = tar.entries().map_err(|e| invalid(format!("not a tar.gz archive: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| invalid(format!("damaged archive: {e}")))?;
        let path = entry.path().map_err(|e| invalid(format!("bad entry name: {e}")))?.into_owned();
        if !stays_inside(&path) {
            return Err(invalid(format!("the entry {} leaves the install directory", path.display())));
        }
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            let target = entry.link_name().ok().flatten().map(|t| t.into_owned()).unwrap_or_default();
            let resolved =
                if kind.is_symlink() { path.parent().unwrap_or(Path::new("")).join(&target) } else { target };
            if !stays_inside(&resolved) {
                return Err(invalid(format!("the link {} points outside the install directory", path.display())));
            }
        } else if !(kind.is_file() || kind.is_dir()) {
            continue; // devices, FIFOs and other special files are never installed
        }
        let unpacked = entry.unpack_in(staging).map_err(|e| io_err("extract into", staging, e))?;
        if !unpacked {
            return Err(invalid(format!("the entry {} was refused", path.display())));
        }
    }
    match strip {
        Some(dir) => {
            let root = staging.join(dir);
            if !root.is_dir() || root.is_symlink() {
                return Err(invalid(format!("the archive has no top-level {dir}/ directory")));
            }
            Ok(root)
        }
        None => Ok(staging.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_must_stay_inside() {
        assert!(stays_inside(Path::new("python/lib/libpython3.12.so.1.0")));
        assert!(stays_inside(Path::new("bin/../lib/x")));
        assert!(!stays_inside(Path::new("../etc/passwd")));
        assert!(!stays_inside(Path::new("/etc/passwd")));
        assert!(!stays_inside(Path::new("a/../../b")));
    }

    #[test]
    fn loopback_urls() {
        assert!(is_loopback_url("http://127.0.0.1:8080/x.tar.gz"));
        assert!(is_loopback_url("http://localhost/x"));
        assert!(is_loopback_url("http://[::1]:8080/x"));
        assert!(!is_loopback_url("https://github.com/x"));
        assert!(!is_loopback_url("http://127.0.0.1:x@example.com/a.tar.gz"), "userinfo is not the host");
        assert!(!is_loopback_url("http://127.0.0.1.example.com/x"));
        assert!(!is_loopback_url("not a url"));
    }

    #[test]
    fn dates_are_rfc3339() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_737), (2026, 10, 11));
        let now = now_rfc3339();
        assert_eq!(now.len(), 20, "{now}");
        assert!(now.ends_with('Z'));
    }
}
