// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Fixes before signing (SIGN §8.5): exec bits, the iOS 27 string-pool alignment, stale signature files, and
//! (opt-in) thinning to arm64. They run on the copy being signed, never on the build product.

use std::path::Path;

use russet_core::Result;
use russet_macho::{Arch, MachO, Version};

use crate::util::{codesign_failed, io_error, slash_path, walk_files};

/// Which pre-sign fixes run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PreSignFixes {
    /// `chmod 0755` on every Mach-O (unpacked IPAs lose exec bits).
    pub chmod: bool,
    /// Pads `LC_SYMTAB` string pools to 8 bytes when `LC_BUILD_VERSION sdk ≥ 27.0` (iOS 27's dyld rule).
    pub align_string_pool: bool,
    /// Removes stale `_CodeSignature/`, `CodeResources`, `.DS_Store` and `__MACOSX`.
    pub clean: bool,
    /// Thins fat Mach-O files to arm64 (off by default; the build plane ships thin binaries).
    pub thin: bool,
}

impl Default for PreSignFixes {
    fn default() -> Self {
        Self { chmod: true, align_string_pool: true, clean: true, thin: false }
    }
}

impl PreSignFixes {
    /// No fix at all.
    pub fn none() -> Self {
        Self { chmod: false, align_string_pool: false, clean: false, thin: false }
    }

    /// Turns one fix off by its name (`chmod`, `align-string-pool`, `clean`, `thin`); `false` for unknown names.
    pub fn disable(&mut self, name: &str) -> bool {
        match name {
            "chmod" => self.chmod = false,
            "align-string-pool" => self.align_string_pool = false,
            "clean" => self.clean = false,
            "thin" => self.thin = false,
            _ => return false,
        }
        true
    }
}

/// One fix that changed a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixRecord {
    /// `chmod`, `align-string-pool`, `clean` or `thin`.
    pub fix: String,
    /// The path relative to the bundle root.
    pub path: String,
}

/// The first SDK version whose binaries must have an 8-byte-aligned string pool.
const ALIGNMENT_SDK: Version = Version::new(27, 0, 0);

/// Applies `fixes` to every file under `root` and returns what changed.
pub fn apply(root: &Path, fixes: PreSignFixes) -> Result<Vec<FixRecord>> {
    let mut records = Vec::new();
    if fixes.clean {
        clean(root, root, &mut records)?;
    }
    for rel in walk_files(root)? {
        let abs = root.join(&rel);
        if abs.is_symlink() || !russet_macho::is_macho_file(&abs).unwrap_or(false) {
            continue;
        }
        let rel_s = slash_path(&rel);
        if fixes.thin {
            let data = std::fs::read(&abs).map_err(|e| io_error("cannot read", &abs, e))?;
            if let Ok(m) = MachO::parse(&data)
                && m.is_fat()
                && m.slices().iter().any(|s| s.arch == Arch::Arm64)
            {
                russet_macho::thin(&abs, Arch::Arm64).map_err(|e| codesign_failed(format!("thinning {rel_s}"), e))?;
                records.push(FixRecord { fix: "thin".into(), path: rel_s.clone() });
            }
        }
        if fixes.align_string_pool {
            let data = std::fs::read(&abs).map_err(|e| io_error("cannot read", &abs, e))?;
            let needs = MachO::parse(&data).is_ok_and(|m| {
                m.images().iter().any(|i| {
                    i.is_64
                        && i.string_pool_alignment() < 8
                        && i.build_version.as_ref().is_some_and(|bv| bv.sdk >= ALIGNMENT_SDK)
                })
            });
            if needs
                && russet_macho::align_string_pool(&abs)
                    .map_err(|e| codesign_failed(format!("aligning the string pool of {rel_s}"), e))?
            {
                records.push(FixRecord { fix: "align-string-pool".into(), path: rel_s.clone() });
            }
        }
        if fixes.chmod {
            chmod_755(&abs, &rel_s, &mut records)?;
        }
    }
    Ok(records)
}

#[cfg(unix)]
fn chmod_755(abs: &Path, rel: &str, records: &mut Vec<FixRecord>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let meta = std::fs::metadata(abs).map_err(|e| io_error("cannot stat", abs, e))?;
    if meta.permissions().mode() & 0o777 != 0o755 {
        std::fs::set_permissions(abs, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| io_error("cannot chmod", abs, e))?;
        records.push(FixRecord { fix: "chmod".into(), path: rel.into() });
    }
    Ok(())
}

#[cfg(not(unix))]
fn chmod_755(_abs: &Path, _rel: &str, _records: &mut Vec<FixRecord>) -> Result<()> {
    // Windows has no exec bit; russet-ipa derives modes from Mach-O magic (build §4.8).
    Ok(())
}

/// Removes stale signature artefacts and Finder/macOS archive litter, recursively.
fn clean(top: &Path, dir: &Path, records: &mut Vec<FixRecord>) -> Result<()> {
    let entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| io_error("cannot read", dir, e))?
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| io_error("cannot read", dir, e))?;
    for e in entries {
        let path = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        let ft = e.file_type().map_err(|err| io_error("cannot stat", &path, err))?;
        let rel = slash_path(path.strip_prefix(top).unwrap_or(&path));
        if ft.is_dir() && (name == "_CodeSignature" || name == "__MACOSX") {
            std::fs::remove_dir_all(&path).map_err(|err| io_error("cannot remove", &path, err))?;
            records.push(FixRecord { fix: "clean".into(), path: rel });
        } else if !ft.is_dir() && (name == ".DS_Store" || (name == "CodeResources" && dir == top)) {
            std::fs::remove_file(&path).map_err(|err| io_error("cannot remove", &path, err))?;
            records.push(FixRecord { fix: "clean".into(), path: rel });
        } else if ft.is_dir() {
            clean(top, &path, records)?;
        }
    }
    Ok(())
}
