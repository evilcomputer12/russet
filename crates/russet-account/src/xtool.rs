// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Reading xtool's credential store (`~/.config/xtool/data`) for the explicit, one-time import [D 50]
//! (SIGN §4.4). Formats, from xtool's `XToolSupport/AuthToken.swift` and `XKit/Utilities/SigningInfo.swift`:
//!
//! - `XTLAuthToken`: JSON of Swift's `enum AuthToken`, `{"appStoreConnect": {"_0": {"id", "issuerID", "pem"}}}`
//!   (or `{"xcode": {"_0": {…}}}` for an Apple ID session, which Russet does not import in Wave 1);
//! - `<identityID>` (for an App Store Connect key, the identity ID is the issuer ID): a property list
//!   `{privateKey: {data: <PEM>}, certificate: <DER>}` written by `PropertyListEncoder`.
//!
//! The values are read into memory, sealed by the caller and never printed.

use std::path::{Path, PathBuf};

use russet_core::Result;
use russet_core::error::{Error, codes};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::fsutil::io_error;

/// xtool's App Store Connect key.
pub struct XtoolAscKey {
    pub key_id: String,
    pub issuer_id: String,
    pub pem: Zeroizing<Vec<u8>>,
}

/// xtool's development identity.
pub struct XtoolIdentity {
    pub cert_der: Vec<u8>,
    pub key_pem: Zeroizing<Vec<u8>>,
}

/// What the store holds.
pub struct XtoolData {
    pub asc_key: Option<XtoolAscKey>,
    pub identity: Option<XtoolIdentity>,
    /// Paths whose mode lets group or others read them (Unix).
    pub world_readable: Vec<PathBuf>,
    /// Every credential file found (for the advice text).
    pub files: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// xtool's data directory: `$XDG_CONFIG_HOME/xtool/data`, else `~/.config/xtool/data`.
pub fn default_dir() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(x).join("xtool/data"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/xtool/data"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum AuthToken {
    AppStoreConnect {
        #[serde(rename = "_0")]
        inner: AscToken,
    },
    Xcode {
        #[serde(rename = "_0")]
        #[allow(dead_code)]
        inner: serde_json::Value,
    },
}

#[derive(Deserialize)]
struct AscToken {
    id: String,
    #[serde(rename = "issuerID")]
    issuer_id: String,
    pem: String,
}

/// Reads xtool's store. Fails with `core.notFound` when xtool has no saved login.
pub fn read(dir: &Path) -> Result<XtoolData> {
    let token_path = dir.join("XTLAuthToken");
    if !token_path.is_file() {
        return Err(Error::new(codes::core::NOT_FOUND, format!("xtool has no saved login in {}", dir.display()))
            .with_hint("Log in with `xtool auth` first, or add the key directly: `russet account add-key --key-id … --issuer-id … --p8 AuthKey.p8`."));
    }
    let raw = Zeroizing::new(std::fs::read(&token_path).map_err(|e| io_error("cannot read", &token_path, e))?);
    let token: AuthToken = serde_json::from_slice(&raw).map_err(|_| {
        Error::new(codes::account::KEY_INVALID, "xtool's XTLAuthToken has an unknown format")
            .with_hint("Update xtool and log in again, or add the key directly with `russet account add-key`.")
    })?;
    let mut data = XtoolData {
        asc_key: None,
        identity: None,
        world_readable: Vec::new(),
        files: vec![token_path.clone()],
        warnings: Vec::new(),
    };
    check_mode(&token_path, &mut data.world_readable);
    let asc = match token {
        AuthToken::AppStoreConnect { inner } => inner,
        AuthToken::Xcode { .. } => {
            data.warnings.push(
                "xtool is logged in with an Apple ID session; Russet imports App Store Connect keys only (free Apple IDs come with W22)".into(),
            );
            return Ok(data);
        }
    };
    // For App Store Connect logins xtool's identity ID is the issuer ID; its file is named after it.
    let identity_path = dir.join(&asc.issuer_id);
    data.asc_key = Some(XtoolAscKey {
        key_id: asc.id,
        issuer_id: asc.issuer_id.clone(),
        pem: Zeroizing::new(asc.pem.into_bytes()),
    });
    if identity_path.is_file() && is_safe_name(&asc.issuer_id) {
        data.files.push(identity_path.clone());
        check_mode(&identity_path, &mut data.world_readable);
        match read_identity(&identity_path) {
            Ok(id) => data.identity = Some(id),
            Err(e) => data.warnings.push(format!("xtool's development identity was not imported: {}", e.message())),
        }
    } else {
        data.warnings.push(
            "xtool has no development identity for this key yet (it creates one on the first `xtool dev`)".into(),
        );
    }
    Ok(data)
}

fn is_safe_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn read_identity(path: &Path) -> Result<XtoolIdentity> {
    let raw = Zeroizing::new(std::fs::read(path).map_err(|e| io_error("cannot read", path, e))?);
    let value = plist::Value::from_reader(std::io::Cursor::new(&raw[..]))
        .map_err(|_| Error::new(codes::sign::NO_IDENTITY, "xtool's identity file is not a property list"))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| Error::new(codes::sign::NO_IDENTITY, "xtool's identity file is not a dictionary"))?;
    let cert_der = dict
        .get("certificate")
        .and_then(plist::Value::as_data)
        .ok_or_else(|| Error::new(codes::sign::NO_IDENTITY, "xtool's identity has no certificate"))?
        .to_vec();
    let key_pem = dict
        .get("privateKey")
        .and_then(plist::Value::as_dictionary)
        .and_then(|d| d.get("data"))
        .and_then(plist::Value::as_data)
        .ok_or_else(|| Error::new(codes::sign::NO_IDENTITY, "xtool's identity has no private key"))?;
    Ok(XtoolIdentity { cert_der, key_pem: Zeroizing::new(key_pem.to_vec()) })
}

#[cfg(unix)]
fn check_mode(path: &Path, out: &mut Vec<PathBuf>) {
    use std::os::unix::fs::PermissionsExt as _;
    if std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o077 != 0) {
        out.push(path.to_path_buf());
    }
}

#[cfg(not(unix))]
fn check_mode(_path: &Path, _out: &mut Vec<PathBuf>) {}
