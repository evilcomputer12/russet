// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! dSYM generation (SIGN §9.1). `dsymutil` runs on the unstripped binaries before stripping; the tool is xtool's
//! toolset `dsymutil` or the system's LLVM `dsymutil`, passed in by the caller (the build area resolves it). The
//! build pipeline already produces dSYMs, so the archive usually copies [`crate::ArchiveRequest::dsyms`]; this is
//! for callers that want to (re)generate them from binaries.

use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;

use crate::archive_failed;

/// Runs `dsymutil` on each binary in `binaries`, writing `<binary file name>.dSYM` into `out_dir`, and returns
/// the created dSYM bundles. `dsymutil` is the absolute path of the tool.
pub fn generate_dsyms(binaries: &[Utf8PathBuf], out_dir: &Utf8Path, dsymutil: &Utf8Path) -> Result<Vec<Utf8PathBuf>> {
    if !dsymutil.is_file() {
        return Err(archive_failed(format!("dsymutil not found at {dsymutil}")));
    }
    std::fs::create_dir_all(out_dir).map_err(|e| archive_failed(format!("cannot create {out_dir}: {e}")))?;
    let mut out = Vec::with_capacity(binaries.len());
    for binary in binaries {
        if !binary.is_file() {
            return Err(archive_failed(format!("binary {binary} does not exist")));
        }
        let name = binary.file_name().ok_or_else(|| archive_failed(format!("{binary} has no file name")))?;
        let dsym = out_dir.join(format!("{name}.dSYM"));
        if dsym.exists() {
            std::fs::remove_dir_all(&dsym).map_err(|e| archive_failed(format!("cannot replace {dsym}: {e}")))?;
        }
        let status = Command::new(dsymutil.as_std_path())
            .arg("--out")
            .arg(dsym.as_std_path())
            .arg(binary.as_std_path())
            .status()
            .map_err(|e| archive_failed(format!("cannot run dsymutil: {e}")))?;
        if !status.success() {
            return Err(archive_failed(format!("dsymutil failed on {binary} (status {status})")));
        }
        out.push(dsym);
    }
    Ok(out)
}
