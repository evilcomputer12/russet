// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `sdk.import`: the native SDK and DDI importer on russet-xar (ARCHITECTURE §5.3 `import`, §16.3)
//! [D 58, D 62, D 69]. Signature checks (`.xip`, `.pkg`) and the SHA-384 manifest-digest check run before
//! anything is stored; `delete_source_after` is honoured only after both parts succeeded and the import
//! manifest verified.
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W16b fills and owns it. `windows` belongs to W27b.

use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::progress::EventSink;
use russet_toolchain::SdkInfo;
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
pub mod windows;

/// Which parts to import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ImportPart {
    Sdk,
    Ddi,
}

/// Which files of the SDK to keep (ARCHITECTURE §16.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WantSet {
    /// The iOS-only want-set (about 421 MiB).
    #[default]
    Ios,
    Full,
}

/// How links are materialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPolicy {
    /// Unix.
    Symlink,
    /// Windows default: directory junctions and file hardlinks.
    JunctionHardlink,
    Copy,
}

/// Where the bundle goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `data/sdk/<id>/bundle`.
    Bundle,
    /// `%LOCALAPPDATA%\Russet\s\<id8>` (Windows).
    ShortRoot,
}

/// An import request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRequest {
    /// `.xip` | `Xcode.app` | `.artifactbundle` | `XcodeSystemResources.pkg` | `iOS_DDI.dmg` | an Apple Device
    /// Support `.dmg` | a DDI directory (a root with `Restore/`, or a `Restore` directory).
    pub source: Utf8PathBuf,
    pub parts: BTreeSet<ImportPart>,
    pub want: WantSet,
    pub links: LinkPolicy,
    pub layout: Layout,
    pub delete_source_after: bool,
    pub dry_run: bool,
}

/// A DDI form found in the BuildManifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DdiForm {
    PersonalizedDmg,
    Cryptex1,
}

/// How the source was verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verified {
    /// xar signature chained to the pinned Apple Root CA.
    AppleSignedPkg,
    /// SHA-384 manifest digests only.
    DigestOnly,
}

/// The DDI half of an import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdiImport {
    Imported {
        /// The DDI's own `ProductBuildVersion`.
        ddi_build: String,
        xcode_build: Option<String>,
        forms: Vec<DdiForm>,
        cryptex_version: Option<String>,
        core_device_version: Option<String>,
        verified: Verified,
        path: Utf8PathBuf,
    },
    Missing {
        reason: String,
    },
    Skipped,
}

/// Link counts of an import.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkCounts {
    pub junctions: u64,
    pub hardlinks: u64,
    pub copies: u64,
    pub symlinks: u64,
}

/// The xar signature check result. W16b defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureReport {
    _f0: (),
}

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub sdk: Option<SdkInfo>,
    pub ddi: DdiImport,
    pub bytes_written: u64,
    pub max_abs_path_len: usize,
    pub links: LinkCounts,
    pub signature: Option<SignatureReport>,
}

/// Runs an import.
pub async fn import(
    r: &ImportRequest,
    dirs: &RussetDirs,
    sink: &dyn EventSink,
    cancel: &CancellationToken,
) -> Result<ImportReport> {
    todo!("W16b")
}
