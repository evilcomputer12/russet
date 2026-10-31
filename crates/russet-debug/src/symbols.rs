// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Device symbols: the cache, status, the consent-gated fetch plan, the quota and LRU prune (RD §11, RD-12).
//! Owner: W11.
//!
//! The symbol cache lives under `cache/device-support/<ProductBuildVersion>/<dsc-uuid>/` (RD §11.2). Russet's
//! own symbolizer and LLDB's `--sysroot` read it. This module owns the on-disk layout, the manifest, and the
//! N3 download rules (confirm the size, keep 2 GiB free, respect the quota); the actual pull from
//! `com.apple.dt.remoteFetchSymbols` is device-gated and needs owner Q13 and tier GREEN (queued as DC-09).
//!
//! A fetch is refused here, before any download, unless all N3 conditions hold, so the refusal is fully
//! host-tested; the device download itself ([`SymbolCache::fetch`]'s transfer step) lands with the device-plane
//! fetch client.

use std::path::{Path, PathBuf};

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::debug::{
    DebugSymbolsFetchResult, DebugSymbolsPlanResult, DebugSymbolsStatusResult, PlanFile, SymbolTier,
};
use serde::{Deserialize, Serialize};

/// Free space a fetch must leave after the download (RD §11.2 N3).
pub const FETCH_HEADROOM: u64 = 2 << 30;
/// The default cache quota (LRU above it).
pub const DEFAULT_QUOTA: u64 = 20 << 30;

/// A file the device offers for download (RD §11.2): a path and its declared length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSymbolFile {
    pub path: String,
    pub size: u64,
}

/// `manifest.json` (RD §11.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default = "one")]
    pub schema: u32,
    #[serde(default)]
    pub product_version: String,
    #[serde(default)]
    pub build_version: String,
    #[serde(default)]
    pub dsc_uuid: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub bytes_on_disk: u64,
    /// RFC 3339; the LRU key for [`SymbolCache::prune`].
    #[serde(default)]
    pub last_used: String,
    #[serde(default)]
    pub raw_dsc_deleted: bool,
}

fn one() -> u32 {
    1
}

/// The device-support symbol cache (`cache/device-support/`).
#[derive(Debug, Clone)]
pub struct SymbolCache {
    root: PathBuf,
    quota: u64,
}

impl SymbolCache {
    /// The cache rooted at Russet's cache directory (ARCHITECTURE §15).
    pub fn new(cache_dir: &Path) -> Self {
        Self { root: cache_dir.join("device-support"), quota: DEFAULT_QUOTA }
    }

    /// Overrides the quota (tests, `[debug] symbol-cache-quota`).
    pub fn with_quota(mut self, quota: u64) -> Self {
        self.quota = quota;
        self
    }

    /// The quota in bytes.
    pub fn quota(&self) -> u64 {
        self.quota
    }

    fn set_dir(&self, build: &str, dsc_uuid: &str) -> PathBuf {
        self.root.join(build).join(dsc_uuid)
    }

    /// Reads the manifest of a build's symbol set, when one is present.
    fn manifest_for(&self, build: &str) -> Option<(PathBuf, Manifest)> {
        let build_dir = self.root.join(build);
        let rd = std::fs::read_dir(&build_dir).ok()?;
        for entry in rd.filter_map(std::result::Result::ok) {
            let manifest = entry.path().join("manifest.json");
            if let Ok(bytes) = std::fs::read(&manifest)
                && let Ok(m) = serde_json::from_slice::<Manifest>(&bytes)
            {
                return Some((entry.path(), m));
            }
        }
        None
    }

    /// `debug.symbols.status`: what the cache holds for `build`.
    pub fn status(&self, build: Option<&str>) -> DebugSymbolsStatusResult {
        match build.and_then(|b| self.manifest_for(b)) {
            Some((dir, m)) => DebugSymbolsStatusResult {
                build: Some(m.build_version.clone()),
                dsc_uuid: (!m.dsc_uuid.is_empty()).then(|| m.dsc_uuid.clone()),
                tier: tier_number(&m.tier),
                bytes_on_disk: dir_size(&dir),
                raw_dsc_present: dir.join("dsc").is_dir() && !m.raw_dsc_deleted,
            },
            None => DebugSymbolsStatusResult {
                build: build.map(str::to_owned),
                dsc_uuid: None,
                tier: 0,
                bytes_on_disk: 0,
                raw_dsc_present: false,
            },
        }
    }

    /// `debug.symbols.plan`: the files and sizes, with the N3 space and quota check. `files` is the device's
    /// offer (empty when the device plane cannot list them yet; then the plan is `ok: false`).
    pub fn plan(&self, files: &[DeviceSymbolFile]) -> Result<DebugSymbolsPlanResult> {
        let total: u64 = files.iter().map(|f| f.size).sum();
        let free = russet_core::fsx::free_space(&self.root).unwrap_or(0);
        let current = self.total_size();
        let (ok, reason) = self.admits(total, free, current);
        Ok(DebugSymbolsPlanResult {
            files: files.iter().map(|f| PlanFile { path: f.path.clone(), size: f.size }).collect(),
            total_bytes: total,
            free_bytes: free,
            quota_bytes: self.quota,
            ok,
            reason,
        })
    }

    /// Whether a download of `total` bytes is allowed given `free` space and the `current` cache size.
    fn admits(&self, total: u64, free: u64, current: u64) -> (bool, Option<String>) {
        if total == 0 {
            return (false, Some("the device offered no symbol files (is it listing them?)".to_owned()));
        }
        if free < total + FETCH_HEADROOM {
            return (
                false,
                Some(format!(
                    "need {} free (download {} + {} headroom), have {}",
                    human(total + FETCH_HEADROOM),
                    human(total),
                    human(FETCH_HEADROOM),
                    human(free)
                )),
            );
        }
        if current + total > self.quota {
            return (
                false,
                Some(format!(
                    "the download ({}) plus the cache ({}) exceeds the {} quota; prune first",
                    human(total),
                    human(current),
                    human(self.quota)
                )),
            );
        }
        (true, None)
    }

    /// Checks the N3 consent and space rules for a fetch, before any download (RD §11.2). Returns the planned
    /// total on success, or `debug.symbolsNoSpace` (6011). `confirm_bytes` must equal the planned total for
    /// headless and MCP callers; `interactive` callers are asked with `ui.confirm` by the handler instead.
    pub fn check_fetch(
        &self,
        files: &[DeviceSymbolFile],
        confirm_bytes: Option<u64>,
        interactive: bool,
    ) -> Result<u64> {
        let total: u64 = files.iter().map(|f| f.size).sum();
        let free = russet_core::fsx::free_space(&self.root).unwrap_or(0);
        let current = self.total_size();
        let (ok, reason) = self.admits(total, free, current);
        if !ok {
            return Err(crate::errors::error(
                codes::debug::SYMBOLS_NO_SPACE,
                reason.unwrap_or_else(|| "the symbol fetch is not allowed".to_owned()),
            )
            .with_details(serde_json::json!({ "totalBytes": total, "freeBytes": free, "quotaBytes": self.quota })));
        }
        if !interactive && confirm_bytes != Some(total) {
            return Err(Error::new(
                codes::perm::CONFIRMATION_UNAVAILABLE,
                format!("confirmBytes must equal the planned total ({total}); run `russet symbols plan` first"),
            )
            .with_hint("Headless and MCP callers must pass confirmBytes equal to the planned total (RD §11.2 N3).")
            .with_details(serde_json::json!({ "totalBytes": total })));
        }
        Ok(total)
    }

    /// Writes the manifest for a freshly extracted set (called by the device-plane fetch once the transfer and
    /// `ipsw dyld extract` finish).
    pub fn record(&self, m: &Manifest) -> Result<DebugSymbolsFetchResult> {
        let dir = self.set_dir(&m.build_version, &m.dsc_uuid);
        crate::fsutil::private_dir(&dir).map_err(|e| crate::errors::io("creating the symbol set directory", e))?;
        let bytes = serde_json::to_vec_pretty(m).unwrap_or_default();
        crate::fsutil::atomic_write(&dir.join("manifest.json"), &bytes)
            .map_err(|e| crate::errors::io("writing the symbol manifest", e))?;
        Ok(DebugSymbolsFetchResult {
            build: m.build_version.clone(),
            dsc_uuid: m.dsc_uuid.clone(),
            tier: tier_number(&m.tier),
            bytes_on_disk: dir_size(&dir),
            path: dir.to_string_lossy().into_owned(),
        })
    }

    /// The sysroot for LLDB's `--sysroot` for a build, when a set is present (RD §11.3).
    pub fn sysroot(&self, build: &str) -> Option<PathBuf> {
        self.manifest_for(build).map(|(dir, _)| dir)
    }

    /// The whole cache's size on disk.
    pub fn total_size(&self) -> u64 {
        dir_size(&self.root)
    }

    /// `debug.symbols.prune`: delete least-recently-used sets until the cache is at most `max_bytes` (the quota
    /// when omitted), or delete one named build. Returns the bytes freed.
    pub fn prune(&self, max_bytes: Option<u64>, build: Option<&str>) -> Result<u64> {
        if let Some(b) = build {
            let Some((dir, _)) = self.manifest_for(b) else { return Ok(0) };
            let freed = dir_size(&dir);
            std::fs::remove_dir_all(&dir).map_err(|e| crate::errors::io("pruning the symbol set", e))?;
            self.clean_empty_parent(&dir);
            return Ok(freed);
        }
        let budget = max_bytes.unwrap_or(self.quota);
        let mut sets = self.all_sets();
        // Least-recently-used first.
        sets.sort_by(|a, b| a.1.last_used.cmp(&b.1.last_used));
        let mut total = self.total_size();
        let mut freed = 0;
        for (dir, _) in sets {
            if total <= budget {
                break;
            }
            let size = dir_size(&dir);
            if std::fs::remove_dir_all(&dir).is_ok() {
                self.clean_empty_parent(&dir);
                total = total.saturating_sub(size);
                freed += size;
            }
        }
        Ok(freed)
    }

    /// Marks a build's set used now (LRU), so a later prune keeps the sets a debugger actually loads.
    pub fn touch(&self, build: &str) {
        if let Some((dir, mut m)) = self.manifest_for(build) {
            m.last_used = crate::logs::now_rfc3339();
            if let Ok(bytes) = serde_json::to_vec_pretty(&m) {
                let _ = crate::fsutil::atomic_write(&dir.join("manifest.json"), &bytes);
            }
        }
    }

    fn all_sets(&self) -> Vec<(PathBuf, Manifest)> {
        let mut out = Vec::new();
        for build in std::fs::read_dir(&self.root).into_iter().flatten().filter_map(std::result::Result::ok) {
            if !build.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            for set in std::fs::read_dir(build.path()).into_iter().flatten().filter_map(std::result::Result::ok) {
                let manifest = set.path().join("manifest.json");
                if let Ok(bytes) = std::fs::read(&manifest)
                    && let Ok(m) = serde_json::from_slice::<Manifest>(&bytes)
                {
                    out.push((set.path(), m));
                }
            }
        }
        out
    }

    fn clean_empty_parent(&self, set_dir: &Path) {
        if let Some(build_dir) = set_dir.parent()
            && build_dir != self.root
            && std::fs::read_dir(build_dir).map(|mut r| r.next().is_none()).unwrap_or(false)
        {
            let _ = std::fs::remove_dir(build_dir);
        }
    }
}

/// `0 | 1 | 2` from a tier name.
fn tier_number(tier: &str) -> u8 {
    match tier {
        "core" => 1,
        "full" => 2,
        _ => 0,
    }
}

/// The wire tier name from the request enum (`core`/`full`; others are not fetchable).
pub fn fetch_tier_name(tier: SymbolTier) -> Option<&'static str> {
    match tier {
        SymbolTier::Core => Some("core"),
        SymbolTier::Full => Some("full"),
        SymbolTier::Auto | SymbolTier::None => None,
    }
}

/// The recursive size of a directory in bytes (0 when missing).
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for entry in rd.filter_map(std::result::Result::ok) {
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(entry.path()),
                Ok(_) => total += entry.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => {}
            }
        }
    }
    total
}

/// A rough human size for messages.
fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn files(sizes: &[u64]) -> Vec<DeviceSymbolFile> {
        sizes.iter().enumerate().map(|(i, &s)| DeviceSymbolFile { path: format!("dsc.{i}"), size: s }).collect()
    }

    #[test]
    fn plan_and_fetch_refuse_without_space_or_confirmation() {
        let base = std::env::temp_dir().join(format!("russet-syms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("device-support")).unwrap();
        // A tiny quota forces the quota branch regardless of the host's free space.
        let cache = SymbolCache::new(&base).with_quota(1024);
        let plan = cache.plan(&files(&[4096])).unwrap();
        assert_eq!(plan.total_bytes, 4096);
        assert!(!plan.ok);
        assert!(plan.reason.unwrap().contains("quota"));
        // check_fetch refuses: no space/quota → symbolsNoSpace.
        let e = cache.check_fetch(&files(&[4096]), Some(4096), false).unwrap_err();
        assert_eq!(e.code(), codes::debug::SYMBOLS_NO_SPACE);
        // With a generous quota, a headless caller still needs a matching confirmBytes.
        let cache = SymbolCache::new(&base).with_quota(u64::MAX);
        let small = files(&[10]);
        let e = cache.check_fetch(&small, None, false).unwrap_err();
        assert_eq!(e.code(), codes::perm::CONFIRMATION_UNAVAILABLE);
        let e = cache.check_fetch(&small, Some(999), false).unwrap_err();
        assert_eq!(e.code(), codes::perm::CONFIRMATION_UNAVAILABLE);
        // A matching confirmBytes (or an interactive caller) passes.
        assert_eq!(cache.check_fetch(&small, Some(10), false).unwrap(), 10);
        assert_eq!(cache.check_fetch(&small, None, true).unwrap(), 10);
        // An empty offer is never ok.
        assert!(!cache.plan(&[]).unwrap().ok);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn record_status_and_prune_lru() {
        let base = std::env::temp_dir().join(format!("russet-syms2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("device-support")).unwrap();
        let cache = SymbolCache::new(&base);
        let make = |build: &str, uuid: &str, last: &str, bytes: usize| {
            let r = cache
                .record(&Manifest {
                    schema: 1,
                    product_version: "27.0".into(),
                    build_version: build.into(),
                    dsc_uuid: uuid.into(),
                    tier: "core".into(),
                    bytes_on_disk: 0,
                    last_used: last.into(),
                    raw_dsc_deleted: true,
                })
                .unwrap();
            // Add payload so the set has a real size.
            std::fs::write(base.join("device-support").join(build).join(uuid).join("payload"), vec![0u8; bytes])
                .unwrap();
            r
        };
        let old = make("24A000", "uuid-old", "2026-01-01T00:00:00.000Z", 5000);
        let _new = make("24A437", "uuid-new", "2026-10-30T00:00:00.000Z", 5000);
        assert_eq!(old.tier, 1);
        let status = cache.status(Some("24A437"));
        assert_eq!(status.build.as_deref(), Some("24A437"));
        assert_eq!(status.tier, 1);
        assert!(status.bytes_on_disk >= 5000);
        assert!(!status.raw_dsc_present);
        assert!(cache.sysroot("24A437").is_some());
        // Prune to ~6000 bytes: the least-recently-used (24A000) goes.
        let freed = cache.prune(Some(6000), None).unwrap();
        assert!(freed >= 5000, "freed {freed}");
        assert!(cache.sysroot("24A000").is_none());
        assert!(cache.sysroot("24A437").is_some());
        // Prune a named build.
        let freed = cache.prune(None, Some("24A437")).unwrap();
        assert!(freed >= 5000);
        assert!(cache.sysroot("24A437").is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
