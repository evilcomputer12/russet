// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The store/development IPA (SIGN §9.3), written through the deterministic `russet-ipa` writer: `Payload/<App>`,
//! sorted entries, fixed timestamps, symlinks kept, deflate level 6 for store IPAs and stored entries for
//! development installs. Two runs over the same bundle give the same bytes.

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_ipa::{IpaMode, IpaOptions};
use russet_sign::AppBundle;

use crate::{ExportMethod, invalid};

/// What [`package_ipa`] produced (SIGN §9.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpaInfo {
    pub path: Utf8PathBuf,
    pub size: u64,
    /// Lowercase hex SHA-256 of the whole IPA.
    pub sha256: String,
    pub bundle_id: String,
    pub short_version: String,
    pub build_version: String,
}

/// Packages `app` as an IPA at `out` for `method` (SIGN §9.3). Store methods deflate; `development` stores
/// entries for fast installs. The output is byte-reproducible for identical inputs (fixed timestamps from
/// `SOURCE_DATE_EPOCH`).
pub fn package_ipa(app: &Utf8Path, method: ExportMethod, out: &Utf8Path) -> Result<IpaInfo> {
    if !app.is_dir() || app.extension() != Some("app") {
        return Err(invalid(format!("{app} is not a .app directory")));
    }
    let bundle = AppBundle::open(app.as_std_path())?;
    let bundle_id = bundle.identifier().ok_or_else(|| invalid(format!("{app} has no CFBundleIdentifier")))?.to_owned();
    let short_version = bundle.info_string("CFBundleShortVersionString").unwrap_or("1.0").to_owned();
    let build_version = bundle.info_string("CFBundleVersion").unwrap_or("1").to_owned();

    let mode = if method.is_store() { IpaMode::Store } else { IpaMode::Dev };
    let report = russet_ipa::write(app, out, IpaOptions::new(mode)).map_err(map_ipa_error)?;
    Ok(IpaInfo {
        path: report.path,
        size: report.bytes,
        sha256: report.sha256,
        bundle_id,
        short_version,
        build_version,
    })
}

/// Maps a `russet-ipa` error to the `ship.ipaInvalid` code (the writer's failures are bad inputs or IO).
fn map_ipa_error(e: russet_ipa::IpaError) -> Error {
    Error::new(codes::ship::IPA_INVALID, format!("the IPA writer failed: {e}"))
        .with_hint("Check that the `.app` is a well-formed bundle and the output path is writable.")
}
