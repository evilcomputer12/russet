// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Personal-team (free) provisioning policy (SA-21; SIGN §7.6): Apple allows 10 new App IDs per rolling 7 days,
//! 3 active sideloaded apps per device and 7-day profiles, and exposes none of the remaining quota. Russet keeps
//! a local ledger of the App IDs it created (`<data>/accounts/<team>/ledger.json`), maps bundle IDs another team
//! owns to `<id>.<TEAMID>` (`bundle-map.json`, extensions and app groups follow), and schedules re-provisioning
//! 24 hours before a profile expires. Nothing here revokes anything.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use russet_core::Result;
use russet_core::error::{Error, codes};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// New App IDs per rolling window.
pub const APP_IDS_PER_WINDOW: usize = 10;
/// The rolling window.
pub const WINDOW: time::Duration = time::Duration::days(7);
/// Active sideloaded apps per device (since iOS 13.3.1).
pub const ACTIVE_APPS_PER_DEVICE: usize = 3;
/// Free provisioning profiles last this long.
pub const PROFILE_LIFETIME: time::Duration = time::Duration::days(7);
/// Re-provision this long before a profile expires.
pub const REFRESH_AHEAD: time::Duration = time::Duration::hours(24);

fn team_dir(data_dir: &Path, team: &str) -> Result<PathBuf> {
    if team.is_empty() || !team.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Error::invalid_argument("a team ID is alphanumeric"));
    }
    Ok(data_dir.join("accounts").join(team))
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(v)
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode {}: {e}", path.display())))?;
    crate::fsutil::write_private(path, &bytes)
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> Result<T> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            Error::new(codes::config::INVALID, format!("{} is invalid: {e}", path.display()))
                .with_hint("It is Russet's own bookkeeping; delete it to start over (Apple keeps the real counts).")
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(Error::io(format!("reading {}", path.display()), e)),
    }
}

/// One App ID Russet created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub bundle_id: String,
    /// Unix seconds.
    pub created_at: i64,
}

/// The App ID quota as far as Russet knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quota {
    pub used: usize,
    pub limit: usize,
    /// When the oldest counted App ID leaves the window (only when the quota is used up).
    pub next_slot_at: Option<OffsetDateTime>,
}

impl Quota {
    pub fn available(&self) -> bool {
        self.used < self.limit
    }
}

/// The App ID ledger of one free team.
#[derive(Debug)]
pub struct Ledger {
    path: PathBuf,
    entries: Vec<LedgerEntry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LedgerFile {
    #[serde(default)]
    app_ids: Vec<LedgerEntry>,
}

impl Ledger {
    pub fn load(data_dir: &Path, team: &str) -> Result<Self> {
        let path = team_dir(data_dir, team)?.join("ledger.json");
        let file: LedgerFile = read_json(&path)?;
        Ok(Self { path, entries: file.app_ids })
    }

    /// Records a new App ID (call after Apple created it).
    pub fn record(&mut self, bundle_id: &str, now: OffsetDateTime) -> Result<()> {
        self.entries.retain(|e| now.unix_timestamp() - e.created_at < WINDOW.whole_seconds() * 4);
        self.entries.push(LedgerEntry { bundle_id: bundle_id.to_owned(), created_at: now.unix_timestamp() });
        write_json(&self.path, &LedgerFile { app_ids: self.entries.clone() })
    }

    /// "App IDs: 3/10 this week, next slot frees <date>".
    pub fn quota(&self, now: OffsetDateTime) -> Quota {
        let since = now.unix_timestamp() - WINDOW.whole_seconds();
        let mut recent: Vec<i64> = self.entries.iter().map(|e| e.created_at).filter(|t| *t > since).collect();
        recent.sort_unstable();
        let used = recent.len();
        let next_slot_at = (used >= APP_IDS_PER_WINDOW)
            .then(|| recent.get(used - APP_IDS_PER_WINDOW).copied())
            .flatten()
            .and_then(|t| OffsetDateTime::from_unix_timestamp(t + WINDOW.whole_seconds()).ok());
        Quota { used, limit: APP_IDS_PER_WINDOW, next_slot_at }
    }
}

/// The bundle IDs a free team uses instead of the requested ones.
#[derive(Debug)]
pub struct BundleMap {
    path: PathBuf,
    team: String,
    map: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BundleMapFile {
    #[serde(default)]
    bundles: BTreeMap<String, String>,
}

impl BundleMap {
    pub fn load(data_dir: &Path, team: &str) -> Result<Self> {
        let path = team_dir(data_dir, team)?.join("bundle-map.json");
        let file: BundleMapFile = read_json(&path)?;
        Ok(Self { path, team: team.to_owned(), map: file.bundles })
    }

    /// The identifier used for `requested` (itself unless it was remapped).
    pub fn resolve<'a>(&'a self, requested: &'a str) -> &'a str {
        self.map.get(requested).map_or(requested, String::as_str)
    }

    /// `<requested>.<TEAMID>`: the identifier to use when Apple says `requested` is not available.
    pub fn fallback(&self, requested: &str) -> String {
        format!("{requested}.{}", self.team)
    }

    /// An extension of an app keeps its suffix under the app's identifier (`<app>.<TEAMID>.<suffix>`).
    pub fn extension(&self, app_requested: &str, extension_requested: &str) -> String {
        let app = self.resolve(app_requested);
        match extension_requested.strip_prefix(app_requested).and_then(|s| s.strip_prefix('.')) {
            Some(suffix) if app != app_requested => format!("{app}.{suffix}"),
            _ => self.resolve(extension_requested).to_owned(),
        }
    }

    /// `group.<id>.<TEAMID>` for an app group of a remapped team.
    pub fn app_group(&self, group: &str) -> String {
        match group.strip_prefix("group.") {
            Some(rest) if !self.map.is_empty() => format!("group.{rest}.{}", self.team),
            _ => group.to_owned(),
        }
    }

    /// Records that `requested` is provisioned as `actual`.
    pub fn insert(&mut self, requested: &str, actual: &str) -> Result<()> {
        self.map.insert(requested.to_owned(), actual.to_owned());
        write_json(&self.path, &BundleMapFile { bundles: self.map.clone() })
    }
}

/// Whether a profile expiring at `expires_at` is due for re-provisioning.
pub fn refresh_due(expires_at: OffsetDateTime, now: OffsetDateTime) -> bool {
    expires_at - REFRESH_AHEAD <= now
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quota_counts_a_rolling_week() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let t0 = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("time");
        let mut l = Ledger::load(tmp.path(), "TEAM01").expect("ledger");
        for i in 0..APP_IDS_PER_WINDOW {
            l.record(&format!("com.example.app{i}"), t0 + time::Duration::hours(i as i64)).expect("record");
        }
        let q = Ledger::load(tmp.path(), "TEAM01").expect("reload").quota(t0 + time::Duration::hours(12));
        assert_eq!((q.used, q.limit, q.available()), (10, 10, false));
        assert_eq!(q.next_slot_at, Some(t0 + WINDOW));
        let later = l.quota(t0 + WINDOW + time::Duration::minutes(30));
        assert_eq!(later.used, 9);
        assert!(later.available());
        assert!(Ledger::load(tmp.path(), "../x").is_err());
    }

    #[test]
    fn remapped_bundles_carry_extensions_and_groups() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut m = BundleMap::load(tmp.path(), "TEAM01").expect("map");
        assert_eq!(m.resolve("com.example.app"), "com.example.app");
        let fb = m.fallback("com.example.app");
        assert_eq!(fb, "com.example.app.TEAM01");
        m.insert("com.example.app", &fb).expect("insert");
        let m = BundleMap::load(tmp.path(), "TEAM01").expect("reload");
        assert_eq!(m.extension("com.example.app", "com.example.app.widget"), "com.example.app.TEAM01.widget");
        assert_eq!(m.app_group("group.com.example.shared"), "group.com.example.shared.TEAM01");
    }

    #[test]
    fn refreshes_are_due_a_day_ahead() {
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("time");
        assert!(!refresh_due(now + time::Duration::hours(25), now));
        assert!(refresh_due(now + time::Duration::hours(23), now));
    }
}
