// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Developer Disk Images (ARCHITECTURE §5.3 `ddi`, §16.3) [D 62, D 82, D 89]. W4: status, `Dir`, mount records;
//! W10: `Imported`, `Pmd3Cache`, `Pmd3AutoMount`, `list_imported`.
//!
//! Every mount Russet performs writes a [`MountRecord`] (`state/devices/<udid>/ddi.json`). `ddi_status()`
//! reports the recorded source while the mounted cryptex version and product build still match the record,
//! otherwise `unknown`. Payload paths always come from the selected BuildManifest identity.

use std::path::PathBuf;

use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::progress::ProgressSink;
use time::OffsetDateTime;

use crate::manager::DeviceHandle;

/// Where a DDI comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdiSource {
    /// `data/ddi/<ddiBuild>/iOS`; newest first when `None`.
    Imported { ddi_build: Option<String> },
    /// A DDI root holding `Restore/` and `version.plist` (for example a copy of a Mac's
    /// `/Library/Developer/DeveloperDiskImages/iOS_DDI`); Russet loads `<dir>/Restore`, and also accepts a
    /// directory that directly contains `BuildManifest.plist` [D 82].
    Dir(PathBuf),
    /// pymobiledevice3's mirror-derived cache; needs the allow-mirror consent.
    Pmd3Cache,
    /// The opt-in pymobiledevice3 subprocess (third-party mirror).
    Pmd3AutoMount { allow_mirror: bool },
}

/// The image form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DdiKind {
    Legacy,
    PersonalizedDmg,
    Cryptex1,
}

/// Where a mounted DDI came from, per the mount record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DdiOrigin {
    Imported,
    Dir,
    Pmd3Cache,
    Pmd3AutoMount,
    Unknown,
}

/// `device.ddi.status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdiStatus {
    pub mounted: bool,
    pub kind: DdiKind,
    pub version: Option<String>,
    pub product_build: Option<String>,
    pub source: DdiOrigin,
}

/// One imported DDI (`data/ddi/index.json`). W10 defines the remaining fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedDdi {
    pub ddi_build: String,
    pub path: PathBuf,
}

/// The last DDI mount Russet performed on a device (`state/devices/<udid>/ddi.json`) [D 89].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountRecord {
    pub source: DdiOrigin,
    pub path: Option<PathBuf>,
    pub ddi_build: String,
    pub cryptex_version: Option<String>,
    pub image_sha384: String,
    pub mounted_at: OffsetDateTime,
}

impl DeviceHandle {
    pub async fn ddi_status(&self) -> Result<DdiStatus> {
        todo!("W4 (DEV-13 slice)")
    }
    pub async fn ddi_mount(&self, src: &DdiSource, force: bool, p: &dyn ProgressSink) -> Result<DdiStatus> {
        todo!("W4 (dir) / W10 (the rest)")
    }
    /// The `[device.ddi]` order of ARCHITECTURE §14.3; lazy; remounts only when missing.
    pub async fn ensure_ddi(&self) -> Result<DdiStatus> {
        todo!("W4 / W10")
    }
}

/// `data/ddi/index.json`, newest `ddiBuild` first (`device.ddi.list`).
pub fn list_imported(dirs: &RussetDirs) -> Result<Vec<ImportedDdi>> {
    todo!("W10")
}
