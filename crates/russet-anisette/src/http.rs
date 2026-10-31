// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! HTTP and file helpers shared by the providers.

use std::path::Path;
use std::time::Duration;

use russet_core::Result;
use russet_core::error::{Error, codes};

/// A client with the platform roots (apple.com, the CDN, anisette-v3 servers).
pub(crate) fn plain_client(timeout: Duration) -> Result<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("russet/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot build the HTTP client: {e}")))
}

/// Writes `bytes` to `path` atomically, the directory 0700 and the file 0600 (Unix).
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| Error::io(format!("writing {}", tmp.display()), e))?;
    set_mode(&tmp, 0o600)?;
    std::fs::rename(&tmp, path).map_err(|e| Error::io(format!("renaming {}", path.display()), e))
}

/// Creates `dir` and its missing parents; `dir` itself ends up 0700 (Unix).
pub(crate) fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;
    set_mode(dir, 0o700)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| Error::io(format!("chmod {}", path.display()), e))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// A `text/x-xml-plist` body.
pub(crate) fn plist_xml(v: &plist::Value) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    plist::to_writer_xml(&mut out, v)
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode a plist request: {e}")))?;
    Ok(out)
}

/// A plist dictionary from string keys.
pub(crate) fn dict<const N: usize>(entries: [(&str, plist::Value); N]) -> plist::Value {
    plist::Value::Dictionary(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}
