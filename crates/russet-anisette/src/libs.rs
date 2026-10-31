// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Library acquisition (SIGN §7.5, [V12]): after the user's consent, fetch only Apple's two ADI libraries out of
//! the Apple Music APK on Apple's CDN by HTTP range, verify them and store them 0700 under
//! `<data>/anisette/libs/<apk-version>/<abi>/`.
//!
//! 1. `HEAD` the APK: `Content-Length` and `x-apple-version-number` (the APK version the pins are keyed by).
//! 2. Read the last 64 KiB for the end-of-central-directory record, then range-fetch the central directory.
//! 3. Range-fetch each library's local header and compressed bytes and inflate them (about 2 MB in all).
//! 4. Check CRC-32, size and SHA-256: against `anisette-pins.json` when Russet knows the version, else recorded on
//!    first use (`tofu.json`) and reported as unpinned.
//!
//! A server that ignores `Range` (answers 200) is refused before its body is read, so a broken CDN never costs the
//! full 142 MB.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::net::Endpoints;
use serde::{Deserialize, Serialize};
use url::Url;

/// The Apple Music APK, relative to `Endpoints.anisette_libs` (`https://apps.mzstatic.com/`).
pub const APK_PATH: &str = "content/android-apple-music-apk/applemusic.apk";

/// The two libraries ADI needs.
pub const LIBRARIES: [&str; 2] = ["libstoreservicescore.so", "libCoreADI.so"];

/// The shipped pin table.
const PINS: &str = include_str!("../anisette-pins.json");

const TAIL: u64 = 64 * 1024;
/// Neither library is near this; a larger entry means the APK is not what Russet expects.
const MAX_LIBRARY: u64 = 32 * 1024 * 1024;
/// The central directory of the 2025 APK is 272 KB [V12].
const MAX_CENTRAL_DIRECTORY: u64 = 8 * 1024 * 1024;

/// An Android ABI with ADI libraries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Abi {
    X86_64,
    Arm64V8a,
}

impl Abi {
    /// This host's ABI, if Apple ships libraries for it.
    pub fn host() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X86_64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::Arm64V8a)
        } else {
            None
        }
    }

    /// The APK's `lib/<abi>/` directory name.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Arm64V8a => "arm64-v8a",
        }
    }
}

/// Downloaded, verified libraries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySet {
    pub apk_version: String,
    pub abi: Abi,
    /// The directory holding both libraries.
    pub dir: PathBuf,
    /// Their hashes are in Russet's pin table (false: trusted on first use).
    pub pinned: bool,
}

/// The user's consent to download Apple's libraries (`account.anisette.fetchLibraries {consent: true}`): the only
/// way to call [`ensure_libraries`].
#[derive(Debug, Clone, Copy)]
pub struct Consent(());

impl Consent {
    /// `consent` must be `true`: the client showed the consent screen of SIGN §7.1 and the user accepted it.
    pub fn given(consent: bool) -> Result<Self> {
        if consent {
            Ok(Self(()))
        } else {
            Err(Error::new(codes::anisette::CONSENT_REQUIRED, "downloading Apple's ADI libraries needs consent")
                .with_hint("Show the consent screen (what is downloaded, from where, why) and call again with `consent: true`."))
        }
    }
}

/// What [`ensure_libraries`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub set: LibrarySet,
    /// HTTP body bytes received (0 when the libraries were already present).
    pub bytes_downloaded: u64,
}

/// `<data>/anisette/libs`.
pub fn libs_root(data_dir: &Path) -> PathBuf {
    data_dir.join("anisette").join("libs")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PinFile {
    #[serde(default)]
    versions: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    >,
}

fn shipped_pins() -> PinFile {
    serde_json::from_str(PINS).unwrap_or_default()
}

/// The shipped pins of `version`/`abi`, if Russet knows them.
fn pins_for(version: &str, abi: Abi) -> Option<std::collections::BTreeMap<String, String>> {
    shipped_pins().versions.get(version)?.get(abi.dir_name()).cloned()
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The libraries installed for `abi` that still verify (the newest version first), if any.
pub fn installed(data_dir: &Path, abi: Abi) -> Option<LibrarySet> {
    let root = libs_root(data_dir);
    let mut versions: Vec<String> = std::fs::read_dir(&root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join(abi.dir_name()).is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    versions.sort_by(|a, b| version_key(b).cmp(&version_key(a)));
    versions.into_iter().find_map(|v| verify_dir(&root.join(&v).join(abi.dir_name()), &v, abi).ok())
}

fn version_key(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit()).filter_map(|p| p.parse().ok()).collect()
}

/// Checks the two libraries in `dir` against the pins of `version` (or its TOFU record).
fn verify_dir(dir: &Path, version: &str, abi: Abi) -> Result<LibrarySet> {
    let expected = match pins_for(version, abi) {
        Some(p) => (p, true),
        None => {
            let text = std::fs::read_to_string(dir.join("tofu.json")).map_err(|e| Error::io("reading tofu.json", e))?;
            let map: std::collections::BTreeMap<String, String> =
                serde_json::from_str(&text).map_err(|_| pin_mismatch(version, "tofu.json is unreadable"))?;
            (map, false)
        }
    };
    for lib in LIBRARIES {
        let bytes = std::fs::read(dir.join(lib)).map_err(|_| libs_missing())?;
        if expected.0.get(lib).map(String::as_str) != Some(sha256_hex(&bytes).as_str()) {
            return Err(pin_mismatch(version, lib));
        }
    }
    Ok(LibrarySet { apk_version: version.to_owned(), abi, dir: dir.to_path_buf(), pinned: expected.1 })
}

pub(crate) fn libs_missing() -> Error {
    Error::new(codes::anisette::LIBS_MISSING, "Apple's ADI libraries are not downloaded").with_hint(
        "Download them once with consent: the sign-in page offers it, or `russet account anisette fetch-libraries`.",
    )
}

fn pin_mismatch(version: &str, what: &str) -> Error {
    Error::new(
        codes::anisette::PIN_MISMATCH,
        format!("Apple's ADI library {what} (APK {version}) does not match its pinned SHA-256"),
    )
    .with_hint("Russet refuses to run it. Delete the anisette libraries (`russet account anisette reset --libraries`) and download them again; if it persists, the CDN served something unexpected.")
}

fn fetch_failed(msg: impl Into<String>) -> Error {
    Error::new(codes::anisette::FETCH_FAILED, msg)
        .with_hint("Check the network and try again; Russet only ever reads about 2 MB of the APK by HTTP range.")
        .retryable(true)
}

/// The APK URL under `base` (`Endpoints.anisette_libs`).
pub fn apk_url(base: &Url) -> Result<Url> {
    base.join(APK_PATH).map_err(|e| Error::invalid_argument(format!("anisetteLibs endpoint: {e}")))
}

struct Ranged<'a> {
    http: &'a reqwest::Client,
    url: &'a Url,
    bytes: u64,
}

impl Ranged<'_> {
    async fn range(&mut self, start: u64, len: u64) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        Endpoints::check(self.url)?;
        let end = start + len - 1;
        let resp = self
            .http
            .get(self.url.clone())
            .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
            .send()
            .await
            .map_err(|e| fetch_failed(format!("range request to the APK failed: {e}")))?;
        if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            // Dropping the response closes the connection before the body (the whole APK) is read.
            return Err(fetch_failed(format!(
                "the CDN answered {} to a range request (206 expected); nothing more is downloaded",
                resp.status().as_u16()
            )));
        }
        let body = resp.bytes().await.map_err(|e| fetch_failed(format!("reading a range of the APK: {e}")))?;
        self.bytes += body.len() as u64;
        if body.len() as u64 != len {
            return Err(fetch_failed(format!("a range of the APK has {} bytes, {len} expected", body.len())));
        }
        Ok(body.to_vec())
    }
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    method: u16,
    crc: u32,
    compressed: u64,
    uncompressed: u64,
    local_offset: u64,
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// `(central directory offset, size)` from the end-of-central-directory record in `tail`.
fn find_eocd(tail: &[u8]) -> Option<(u64, u64)> {
    if tail.len() < 22 {
        return None;
    }
    (0..=tail.len() - 22).rev().find_map(|i| {
        if le32(tail, i)? != 0x0605_4b50 {
            return None;
        }
        let comment = le16(tail, i + 20)? as usize;
        if i + 22 + comment != tail.len() {
            return None;
        }
        let size = le32(tail, i + 12)?;
        let offset = le32(tail, i + 16)?;
        (size != u32::MAX && offset != u32::MAX).then_some((u64::from(offset), u64::from(size)))
    })
}

fn parse_central_directory(cd: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(sig) = le32(cd, at) {
        if sig != 0x0201_4b50 {
            break;
        }
        let (Some(method), Some(crc), Some(comp), Some(uncomp), Some(n), Some(m), Some(k), Some(off)) = (
            le16(cd, at + 10),
            le32(cd, at + 16),
            le32(cd, at + 20),
            le32(cd, at + 24),
            le16(cd, at + 28),
            le16(cd, at + 30),
            le16(cd, at + 32),
            le32(cd, at + 42),
        ) else {
            break;
        };
        let Some(name) = cd.get(at + 46..at + 46 + n as usize) else { break };
        out.push(Entry {
            name: String::from_utf8_lossy(name).into_owned(),
            method,
            crc,
            compressed: u64::from(comp),
            uncompressed: u64::from(uncomp),
            local_offset: u64::from(off),
        });
        at += 46 + n as usize + m as usize + k as usize;
    }
    out
}

/// Fetches, verifies and stores both libraries for `abi` unless a verified copy of the CDN's current version is
/// already installed. `data_dir` is Russet's data directory.
pub async fn ensure_libraries(
    data_dir: &Path,
    apk: &Url,
    abi: Abi,
    _consent: Consent,
    http: &reqwest::Client,
) -> Result<Fetched> {
    Endpoints::check(apk)?;
    let head = http.head(apk.clone()).send().await.map_err(|e| fetch_failed(format!("HEAD of the APK failed: {e}")))?;
    if !head.status().is_success() {
        return Err(fetch_failed(format!("the CDN answered {} to HEAD of the APK", head.status().as_u16())));
    }
    let header = |name: &str| head.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let length: u64 = header("content-length")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| fetch_failed("the CDN did not report the APK's length"))?;
    let version = header("x-apple-version-number")
        .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .or_else(|| header("etag").map(|e| format!("etag-{}", &sha256_hex(e.as_bytes())[..12])))
        .ok_or_else(|| fetch_failed("the CDN reported neither a version nor an ETag for the APK"))?;
    let dir = libs_root(data_dir).join(&version).join(abi.dir_name());
    if let Ok(set) = verify_dir(&dir, &version, abi) {
        return Ok(Fetched { set, bytes_downloaded: 0 });
    }

    let mut r = Ranged { http, url: apk, bytes: 0 };
    let tail_len = TAIL.min(length);
    let tail = r.range(length - tail_len, tail_len).await?;
    let (cd_offset, cd_size) =
        find_eocd(&tail).ok_or_else(|| fetch_failed("the APK has no readable end-of-central-directory record"))?;
    if cd_size > MAX_CENTRAL_DIRECTORY || cd_offset + cd_size > length {
        return Err(fetch_failed("the APK's central directory is not where Russet expects it"));
    }
    let tail_start = length - tail_len;
    let cd = if cd_offset >= tail_start {
        let from = (cd_offset - tail_start) as usize;
        tail.get(from..from + cd_size as usize).map(<[u8]>::to_vec).unwrap_or_default()
    } else {
        r.range(cd_offset, cd_size).await?
    };
    let entries = parse_central_directory(&cd);

    let pins = pins_for(&version, abi);
    let mut hashes = std::collections::BTreeMap::new();
    let mut files = Vec::new();
    for lib in LIBRARIES {
        let wanted = format!("lib/{}/{lib}", abi.dir_name());
        let e = entries
            .iter()
            .find(|e| e.name == wanted)
            .ok_or_else(|| fetch_failed(format!("the APK (version {version}) has no {wanted}")))?;
        if e.compressed > MAX_LIBRARY || e.uncompressed > MAX_LIBRARY {
            return Err(fetch_failed(format!("{wanted} in the APK is larger than any known ADI library")));
        }
        let local = r.range(e.local_offset, 30).await?;
        if le32(&local, 0) != Some(0x0403_4b50) {
            return Err(fetch_failed(format!("{wanted} has no local header where the central directory says")));
        }
        let skip = 30 + u64::from(le16(&local, 26).unwrap_or(0)) + u64::from(le16(&local, 28).unwrap_or(0));
        let data = r.range(e.local_offset + skip, e.compressed).await?;
        let bytes = match e.method {
            0 => data,
            8 => {
                let mut out = Vec::with_capacity(e.uncompressed as usize);
                flate2::read::DeflateDecoder::new(&data[..])
                    .take(MAX_LIBRARY + 1)
                    .read_to_end(&mut out)
                    .map_err(|err| fetch_failed(format!("cannot inflate {wanted}: {err}")))?;
                out
            }
            m => {
                return Err(fetch_failed(format!(
                    "{wanted} uses zip method {m}; Russet reads stored or deflated entries"
                )));
            }
        };
        let mut crc = flate2::Crc::new();
        crc.update(&bytes);
        if bytes.len() as u64 != e.uncompressed || crc.sum() != e.crc {
            return Err(fetch_failed(format!("{wanted} failed its CRC-32 or size check")));
        }
        let hash = sha256_hex(&bytes);
        if let Some(p) = &pins
            && p.get(lib) != Some(&hash)
        {
            return Err(pin_mismatch(&version, lib));
        }
        hashes.insert(lib.to_owned(), hash);
        files.push((lib, bytes));
    }

    crate::http::create_private_dir(&dir)?;
    for (lib, bytes) in &files {
        crate::http::write_private(&dir.join(lib), bytes)?;
    }
    if pins.is_none() {
        let record = serde_json::to_vec_pretty(&hashes).unwrap_or_default();
        crate::http::write_private(&dir.join("tofu.json"), &record)?;
        tracing::warn!(apk_version = %version, "anisette libraries trusted on first use (not in Russet's pin table)");
    }
    let set = verify_dir(&dir, &version, abi)?;
    Ok(Fetched { set, bytes_downloaded: r.bytes })
}

/// Deletes every downloaded library (`reset --libraries`).
pub fn remove_all(data_dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(libs_root(data_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io("removing the anisette libraries", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_pin_table_parses() {
        assert!(serde_json::from_str::<PinFile>(PINS).is_ok());
    }

    #[test]
    fn eocd_is_found_with_a_comment() {
        let mut tail = vec![0u8; 100];
        tail.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        tail.extend_from_slice(&[0; 8]);
        tail.extend_from_slice(&50u32.to_le_bytes());
        tail.extend_from_slice(&7u32.to_le_bytes());
        tail.extend_from_slice(&3u16.to_le_bytes());
        tail.extend_from_slice(b"abc");
        assert_eq!(find_eocd(&tail), Some((7, 50)));
        assert_eq!(find_eocd(&tail[..tail.len() - 1]), None);
    }

    #[test]
    fn versions_sort_numerically() {
        assert!(version_key("4.10.0.1") > version_key("4.9.6.1447"));
    }

    #[test]
    fn consent_must_be_true() {
        assert_eq!(Consent::given(false).err().map(|e| e.code()), Some(codes::anisette::CONSENT_REQUIRED));
        assert!(Consent::given(true).is_ok());
    }
}
