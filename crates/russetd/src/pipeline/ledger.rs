// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The install ledger (ARCHITECTURE §14.8): `state/devices/<udid>/installs.json` =
//! `{"<bundleId>": {"cdhash": "…", "installedAt": "…"}}`. `run.start {install: if-changed}` installs only when the
//! signed app's CDHash differs from the recorded one, so a second run with no changes skips the install.
//!
//! The key is the CDHash of the signed main executable ([`russet_debug::macho`]), not a file hash: the CMS
//! signature inside the file carries its signing time, so the file changes on every signing, while the code
//! directory (code, `Info.plist`, resource seal, entitlements) changes only when the app does. An app whose
//! CDHash is unknown (no code signature found) is always installed, and its record is dropped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::run::InstallMode;
use serde::{Deserialize, Serialize};

/// One ledger record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub cdhash: String,
    /// RFC 3339 UTC.
    pub installed_at: String,
}

/// The ledger of every device under `state/devices/`.
#[derive(Debug)]
pub struct InstallLedger {
    root: PathBuf,
}

/// One lock for every ledger instance: handlers build a pipeline per request.
static LEDGER_LOCK: Mutex<()> = Mutex::new(());

impl InstallLedger {
    /// A ledger under `state_dir/devices/`.
    pub fn new(state_dir: &Path) -> Self {
        Self { root: state_dir.join("devices") }
    }

    fn path(&self, udid: &str) -> PathBuf {
        self.root.join(sanitize(udid)).join("installs.json")
    }

    fn read(&self, udid: &str) -> BTreeMap<String, LedgerEntry> {
        std::fs::read(self.path(udid)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// The record for `bundle_id` on `udid`.
    pub fn get(&self, udid: &str, bundle_id: &str) -> Option<LedgerEntry> {
        let _g = LEDGER_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.read(udid).remove(bundle_id)
    }

    /// Whether the pipeline must install: `always` does, `never` does not, `if-changed` when the CDHash differs
    /// from the record or is unknown (`None`).
    pub fn needs_install(&self, udid: &str, bundle_id: &str, cdhash: Option<&str>, mode: InstallMode) -> bool {
        match mode {
            InstallMode::Always => true,
            InstallMode::Never => false,
            InstallMode::IfChanged => match cdhash {
                Some(h) => self.get(udid, bundle_id).is_none_or(|e| e.cdhash != h),
                None => true,
            },
        }
    }

    /// Records an install; an unknown CDHash drops the record, so the next run installs again.
    pub fn record(&self, udid: &str, bundle_id: &str, cdhash: Option<&str>) -> Result<()> {
        let Some(cdhash) = cdhash else { return self.forget(udid, bundle_id) };
        let _g = LEDGER_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut all = self.read(udid);
        all.insert(
            bundle_id.to_owned(),
            LedgerEntry { cdhash: cdhash.to_owned(), installed_at: russet_debug::logs::now_rfc3339() },
        );
        self.write(udid, &all)
    }

    /// Drops the record (after an uninstall, or when an install failed half-way).
    pub fn forget(&self, udid: &str, bundle_id: &str) -> Result<()> {
        let _g = LEDGER_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut all = self.read(udid);
        if all.remove(bundle_id).is_some() {
            self.write(udid, &all)?;
        }
        Ok(())
    }

    fn write(&self, udid: &str, all: &BTreeMap<String, LedgerEntry>) -> Result<()> {
        let path = self.path(udid);
        let io = |e: std::io::Error| Error::new(codes::core::IO, format!("writing the install ledger: {e}"));
        let dir = path.parent().ok_or_else(|| Error::new(codes::core::INTERNAL, "ledger path without a parent"))?;
        std::fs::create_dir_all(dir).map_err(io)?;
        let bytes = serde_json::to_vec_pretty(all)
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("encoding the install ledger: {e}")))?;
        let tmp = dir.join(format!(".installs.json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)
    }
}

/// Keeps a UDID usable as one path component.
fn sanitize(udid: &str) -> String {
    udid.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn if_changed_installs_only_new_hashes() {
        let dir = std::env::temp_dir().join(format!("russet-ledger-{}", std::process::id()));
        let l = InstallLedger::new(&dir);
        let udid = "00008101-0000000000000000";
        let probe = "dev.russet.test.probe";
        assert!(l.needs_install(udid, probe, Some("aa"), InstallMode::IfChanged));
        l.record(udid, probe, Some("aa")).unwrap();
        assert!(!l.needs_install(udid, probe, Some("aa"), InstallMode::IfChanged));
        assert!(l.needs_install(udid, probe, Some("bb"), InstallMode::IfChanged));
        assert!(l.needs_install(udid, probe, Some("aa"), InstallMode::Always));
        assert!(!l.needs_install(udid, probe, Some("bb"), InstallMode::Never));
        // An unknown CDHash always installs.
        assert!(l.needs_install(udid, probe, None, InstallMode::IfChanged));
        // Another device has its own ledger.
        assert!(l.needs_install("other", probe, Some("aa"), InstallMode::IfChanged));
        let file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("devices").join(udid).join("installs.json")).unwrap())
                .unwrap();
        assert_eq!(file["dev.russet.test.probe"]["cdhash"], "aa");
        assert!(file["dev.russet.test.probe"]["installedAt"].as_str().unwrap().ends_with('Z'));
        l.forget(udid, probe).unwrap();
        assert!(l.get(udid, probe).is_none());
        // Installing a build whose CDHash is unknown drops the record.
        l.record(udid, probe, Some("aa")).unwrap();
        l.record(udid, probe, None).unwrap();
        assert!(l.get(udid, probe).is_none());
        assert_eq!(sanitize("../x"), "___x");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
