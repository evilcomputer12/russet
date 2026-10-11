// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The host Swift toolchain a build runs with, and the LLVM rewriters (`llvm-strip`, `llvm-lipo`, `llvm-otool`,
//! `llvm-install-name-tool`, which must be LLVM ≥ 23, build §3.4).
//!
//! In the daemon W3's resolver supplies the Swift toolchain (`toolchain.resolve swift`, ARCHITECTURE §16.1) and
//! [`HostToolchain::from_swift`] wraps its path; [`HostToolchain::probe`] is the standalone fallback that finds
//! `swift` on `PATH`. Both run `swift --version` once.

use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::Version;

/// LLVM rewriters older than this can write LINKEDIT string pools that iOS 27's dyld refuses (build §3.4).
pub const MIN_LLVM_REWRITER: u32 = 23;

/// The Swift toolchain and the LLVM rewriters found on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostToolchain {
    /// The `swift` driver.
    pub swift: Utf8PathBuf,
    /// The directory that holds `swift`, `swiftc` and `clang` (after resolving symlinks).
    pub bin_dir: Utf8PathBuf,
    pub version: Option<Version>,
    /// The first line of `swift --version`, for example `Swift version 6.4 (swift-6.4-RELEASE)`.
    pub version_text: String,
    pub llvm_strip: Option<Utf8PathBuf>,
    pub llvm_lipo: Option<Utf8PathBuf>,
    pub llvm_otool: Option<Utf8PathBuf>,
    pub llvm_install_name_tool: Option<Utf8PathBuf>,
}

impl HostToolchain {
    /// Finds `swift` on `PATH` and the LLVM ≥ 23 rewriters next to it or on `PATH`.
    pub fn probe() -> Result<Self> {
        let swift = which("swift").ok_or_else(|| {
            Error::new(codes::toolchain::MISSING, "no `swift` on PATH")
                .with_hint("Install Swift 6.4 (swift.org, or AUR `swift-bin`) and run `russet doctor`.")
        })?;
        Self::from_swift(&swift)
    }

    /// Wraps a known `swift` driver (W3's resolved `ToolInfo.path`).
    pub fn from_swift(swift: &Utf8Path) -> Result<Self> {
        let real = std::fs::canonicalize(swift)
            .ok()
            .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
            .unwrap_or_else(|| swift.to_owned());
        let bin_dir = real.parent().map(Utf8Path::to_owned).unwrap_or_default();
        let out = Command::new(swift.as_std_path())
            .arg("--version")
            .output()
            .map_err(|e| Error::new(codes::toolchain::BROKEN, format!("cannot run {swift} --version")).caused_by(e))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let version_text = text.lines().find(|l| l.contains("Swift version")).unwrap_or_default().trim().to_owned();
        let version = parse_swift_version(&version_text);
        let llvm = |name: &str| {
            let candidate = bin_dir.join(name);
            let found = if candidate.is_file() { Some(candidate) } else { which(name) };
            found.filter(|p| llvm_major(p).is_some_and(|m| m >= MIN_LLVM_REWRITER))
        };
        Ok(Self {
            swift: swift.to_owned(),
            version,
            version_text,
            llvm_strip: llvm("llvm-strip"),
            llvm_lipo: llvm("llvm-lipo"),
            llvm_otool: llvm("llvm-otool"),
            llvm_install_name_tool: llvm("llvm-install-name-tool"),
            bin_dir,
        })
    }

    /// `swiftc` next to `swift`.
    pub fn swiftc(&self) -> Utf8PathBuf {
        let sibling = self.swift.with_file_name(exe("swiftc"));
        if sibling.is_file() { sibling } else { self.bin_dir.join(exe("swiftc")) }
    }

    /// `clang` from the Swift toolchain, when it has one.
    pub fn clang(&self) -> Option<Utf8PathBuf> {
        Some(self.bin_dir.join(exe("clang"))).filter(|p| p.is_file())
    }
}

/// `Swift version 6.4 (swift-6.4-RELEASE)` → 6.4; also accepts `Apple Swift version 6.4.1 …`.
pub fn parse_swift_version(line: &str) -> Option<Version> {
    let after = line.split("Swift version").nth(1)?;
    Version::parse(after.split_whitespace().next()?)
}

/// The major version from `<tool> --version` (`LLVM version 23.1.1`).
fn llvm_major(tool: &Utf8Path) -> Option<u32> {
    let out = Command::new(tool.as_std_path()).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let after = text.split("LLVM version").nth(1)?;
    after.split_whitespace().next()?.split('.').next()?.parse().ok()
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() }
}

/// The first `name` on `PATH`.
pub fn which(name: &str) -> Option<Utf8PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(exe(name));
        if candidate.is_file() { Utf8PathBuf::from_path_buf(candidate).ok() } else { None }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swift_version_lines_parse() {
        assert_eq!(parse_swift_version("Swift version 6.4 (swift-6.4-RELEASE)"), Some(Version::new(6, 4, 0)));
        assert_eq!(
            parse_swift_version("Apple Swift version 6.4.1 effective-5.10 (swiftlang-6.4.1.2)"),
            Some(Version::new(6, 4, 1))
        );
        assert_eq!(parse_swift_version("clang version 21"), None);
    }
}
