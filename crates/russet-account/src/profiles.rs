// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The profile cache (SIGN §6.4): `data/profiles/<uuid>.mobileprovision` plus `index.json` (ARCHITECTURE §14.8).
//! `provision.apply` fills it; `sign.run` picks profiles from it when none is given.

use std::path::{Path, PathBuf};
use std::time::Duration;

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_sign::{Distribution, ProfileKind, ProvisioningProfile};
use serde::{Deserialize, Serialize};

use crate::fsutil::{io_error, rfc3339, write_private};

/// One cached profile, as `index.json` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedProfile {
    pub uuid: String,
    pub name: String,
    pub team: String,
    /// The bundle part of the App ID (`com.example.app`, `*`).
    pub bundle: String,
    /// `development`, `ad-hoc`, `app-store`, `in-house`.
    pub kind: String,
    pub expires: String,
    pub cert_serials: Vec<String>,
    pub device_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asc_id: Option<String>,
}

/// What a signer needs from a profile.
#[derive(Debug, Clone, Default)]
pub struct ProfileQuery<'a> {
    pub bundle_id: &'a str,
    pub team: &'a str,
    pub cert_der: &'a [u8],
    pub distribution: Distribution,
    /// A device the profile must list (development and ad hoc).
    pub device: Option<&'a str>,
}

/// The profile kind a distribution signs with.
pub fn kind_for(d: Distribution) -> ProfileKind {
    match d {
        Distribution::Development => ProfileKind::Development,
        Distribution::AdHoc => ProfileKind::AdHoc,
        Distribution::AppStore => ProfileKind::AppStore,
    }
}

/// Profiles cached on disk.
#[derive(Debug, Clone)]
pub struct ProfileCache {
    dir: PathBuf,
}

impl ProfileCache {
    /// The cache under `data_dir/profiles`.
    pub fn new(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("profiles") }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file of a cached profile.
    pub fn path_of(&self, uuid: &str) -> PathBuf {
        self.dir.join(format!("{uuid}.mobileprovision"))
    }

    /// Parses and stores a profile (replacing one with the same UUID); returns it and its path.
    pub fn put(&self, raw: &[u8], asc_id: Option<&str>) -> Result<(ProvisioningProfile, PathBuf)> {
        let p = ProvisioningProfile::parse(raw)?;
        if !p.uuid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || p.uuid.is_empty() {
            return Err(Error::new(codes::sign::PROFILE_INVALID, "the profile's UUID is not usable as a file name")
                .with_hint("The profile is malformed: download it again from the developer portal, or recreate it (`russet provision`)."));
        }
        let path = self.path_of(&p.uuid);
        write_private(&path, raw)?;
        let mut index = self.index()?;
        index.retain(|e| e.uuid != p.uuid);
        index.push(entry_for(&p, asc_id));
        index.sort_by(|a, b| a.uuid.cmp(&b.uuid));
        let json = serde_json::to_vec_pretty(&index)
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode the profile index: {e}")))?;
        write_private(&self.dir.join("index.json"), &json)?;
        Ok((p, path))
    }

    /// The index (rebuilt from the files when it is missing).
    pub fn index(&self) -> Result<Vec<CachedProfile>> {
        match std::fs::read(self.dir.join("index.json")) {
            Ok(raw) => Ok(serde_json::from_slice(&raw).unwrap_or_default()),
            Err(_) => Ok(self.scan()?.into_iter().map(|(p, _)| entry_for(&p, None)).collect()),
        }
    }

    /// Every cached profile, parsed.
    pub fn scan(&self) -> Result<Vec<(ProvisioningProfile, PathBuf)>> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Ok(out) };
        for e in entries.filter_map(|e| e.ok()) {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "mobileprovision") {
                let raw = std::fs::read(&path).map_err(|err| io_error("cannot read", &path, err))?;
                if let Ok(p) = ProvisioningProfile::parse(&raw) {
                    out.push((p, path));
                }
            }
        }
        Ok(out)
    }

    /// The best cached profile for a bundle: it covers the bundle and team, lists the certificate (and the
    /// device), has the distribution's kind and is valid for at least a day; an exact App ID beats a wildcard,
    /// then the latest expiry wins.
    pub fn select(&self, q: &ProfileQuery<'_>) -> Result<Option<(ProvisioningProfile, PathBuf)>> {
        let kind = kind_for(q.distribution);
        let mut matches: Vec<(ProvisioningProfile, PathBuf)> = self
            .scan()?
            .into_iter()
            .filter(|(p, _)| {
                p.kind == kind
                    && p.covers(q.bundle_id, q.team)
                    && p.has_certificate(q.cert_der)
                    && !p.expires_within(Duration::from_secs(24 * 3600))
                    && q.device.is_none_or(|d| p.has_device(d))
            })
            .collect();
        matches.sort_by(|(a, _), (b, _)| a.is_wildcard().cmp(&b.is_wildcard()).then(b.expiration.cmp(&a.expiration)));
        Ok(matches.into_iter().next())
    }
}

fn entry_for(p: &ProvisioningProfile, asc_id: Option<&str>) -> CachedProfile {
    CachedProfile {
        uuid: p.uuid.clone(),
        name: p.name.clone(),
        team: p.team_id.clone(),
        bundle: p.bundle_id_pattern().to_owned(),
        kind: p.kind.as_str().to_owned(),
        expires: rfc3339(p.expiration),
        cert_serials: p
            .developer_certificates
            .iter()
            .filter_map(|c| russet_sign::certificate_info(c).ok().map(|i| i.serial))
            .collect(),
        device_count: p.provisioned_devices.as_ref().map_or(0, Vec::len),
        asc_id: asc_id.map(str::to_owned),
    }
}
