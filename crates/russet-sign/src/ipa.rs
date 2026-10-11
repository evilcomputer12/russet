// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Reading inputs: an `.app` directory as is, or an `.ipa` unpacked to a temporary directory (SIGN §8.1).
//! Unpacking refuses absolute and `..` names, never writes through a symlink, and caps the total size.

use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::util::{bundle_invalid, io_error};

/// Total uncompressed size an `.ipa` may unpack to.
const MAX_UNPACKED_BYTES: u64 = 8 << 30;

/// An opened input: the `.app` to read, plus the temporary directory holding an unpacked `.ipa`.
#[derive(Debug)]
pub struct OpenedInput {
    app: PathBuf,
    ipa_root: Option<PathBuf>,
    _tmp: Option<tempfile::TempDir>,
}

impl OpenedInput {
    /// The `.app` directory.
    pub fn app(&self) -> &Path {
        &self.app
    }

    /// Whether the input was an `.ipa`.
    pub fn is_ipa(&self) -> bool {
        self.ipa_root.is_some()
    }

    /// The unpacked `.ipa` root (holding `Payload/` and, for store IPAs, `SwiftSupport/`).
    pub fn ipa_root(&self) -> Option<&Path> {
        self.ipa_root.as_deref()
    }
}

/// Opens an `.app` directory or unpacks an `.ipa`.
pub fn open_input(path: &Path) -> Result<OpenedInput> {
    if path.is_dir() {
        return Ok(OpenedInput { app: path.to_path_buf(), ipa_root: None, _tmp: None });
    }
    if !path.is_file() {
        return Err(Error::new(codes::core::NOT_FOUND, format!("{} does not exist", path.display()))
            .with_hint("Pass the path of a built .app directory or .ipa file."));
    }
    let tmp = tempfile::Builder::new()
        .prefix("russet-ipa-")
        .tempdir()
        .map_err(|e| io_error("cannot create a temporary directory for", path, e))?;
    unpack(path, tmp.path())?;
    let payload = tmp.path().join("Payload");
    let apps: Vec<PathBuf> = std::fs::read_dir(&payload)
        .map_err(|_| bundle_invalid(path, "the .ipa has no Payload/ directory"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "app") && p.is_dir())
        .collect();
    match apps.as_slice() {
        [app] => Ok(OpenedInput { app: app.clone(), ipa_root: Some(tmp.path().to_path_buf()), _tmp: Some(tmp) }),
        [] => Err(bundle_invalid(path, "Payload/ holds no .app")),
        _ => Err(bundle_invalid(path, "Payload/ holds more than one .app")),
    }
}

fn unpack(ipa: &Path, dest: &Path) -> Result<()> {
    let invalid = |why: String| {
        Error::new(codes::sign::BUNDLE_INVALID, format!("{} is not a valid .ipa: {why}", ipa.display()))
            .with_hint("Rebuild the IPA (`russet package ipa`), or pass the .app directory.")
    };
    let file = std::fs::File::open(ipa).map_err(|e| io_error("cannot open", ipa, e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| invalid(e.to_string()))?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| invalid(e.to_string()))?;
        let name = entry.enclosed_name().ok_or_else(|| invalid(format!("unsafe entry name {:?}", entry.name())))?;
        if name.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(invalid(format!("unsafe entry name {}", name.display())));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_UNPACKED_BYTES {
            return Err(invalid("it unpacks to more than 8 GiB".into()));
        }
        let out = dest.join(&name);
        if !no_symlink_on_path(dest, &out) {
            return Err(invalid(format!("{} would be written through a symlink", name.display())));
        }
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| io_error("cannot create", &out, e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_error("cannot create", parent, e))?;
        }
        if entry.is_symlink() {
            let mut target = String::new();
            entry.read_to_string(&mut target).map_err(|e| invalid(e.to_string()))?;
            let t = Path::new(&target);
            let escapes = t.is_absolute()
                || t.components().filter(|c| matches!(c, Component::ParentDir)).count() >= name.components().count();
            if escapes {
                return Err(invalid(format!("symlink {} points outside the archive", name.display())));
            }
            make_symlink(t, &out)?;
            continue;
        }
        let mut f = std::fs::File::create(&out).map_err(|e| io_error("cannot create", &out, e))?;
        std::io::copy(&mut entry, &mut f).map_err(|e| invalid(format!("{}: {e}", name.display())))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o755));
        }
    }
    Ok(())
}

/// Whether no existing component between `root` and `path` is a symlink.
fn no_symlink_on_path(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else { return false };
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        cur.push(c);
        if cur.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
            return false;
        }
    }
    true
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|e| io_error("cannot create the symlink", link, e))
}

#[cfg(not(unix))]
fn make_symlink(_target: &Path, link: &Path) -> Result<()> {
    Err(Error::new(codes::core::UNSUPPORTED_PLATFORM, format!("cannot create the symlink {}", link.display()))
        .with_hint("Unpack this .ipa on Linux or macOS; symbolic links inside bundles are not supported here."))
}
