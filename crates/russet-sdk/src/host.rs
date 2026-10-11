// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The host Swift toolchain a build runs with, and the LLVM rewriters (`llvm-strip`, `llvm-lipo`, `llvm-otool`,
//! `llvm-install-name-tool`, which must be LLVM ≥ 23, build §3.4).
//!
//! In the daemon W3's resolver supplies the Swift toolchain (`toolchain.resolve swift`, ARCHITECTURE §16.1) and
//! [`HostToolchain::from_swift`] wraps its path; [`HostToolchain::probe`] is the standalone fallback that finds
//! `swift` on `PATH`. The `--version` probes run through the caller's [`CommandRunner`] (the daemon's
//! `ProcessRunner`, contract T1 [D 73]), each bounded by [`PROBE_TIMEOUT`], so a hung tool cannot hang a resolve.

use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::proc::{CommandRunner, CommandSpec};
use russet_core::progress::{EventSink, JobEventKind, LogStream, ProgressSink};
use tokio_util::sync::CancellationToken;

use crate::Version;

/// LLVM rewriters older than this can write LINKEDIT string pools that iOS 27's dyld refuses (build §3.4).
pub const MIN_LLVM_REWRITER: u32 = 23;

/// How long one `<tool> --version` probe may run before it is killed.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

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
    pub async fn probe(runner: &dyn CommandRunner) -> Result<Self> {
        let swift = which("swift").ok_or_else(|| {
            Error::new(codes::toolchain::MISSING, "no `swift` on PATH")
                .with_hint("Install Swift 6.4 (swift.org, or AUR `swift-bin`) and run `russet doctor`.")
        })?;
        Self::from_swift(runner, &swift).await
    }

    /// Wraps a known `swift` driver (W3's resolved `ToolInfo.path`).
    pub async fn from_swift(runner: &dyn CommandRunner, swift: &Utf8Path) -> Result<Self> {
        let real = std::fs::canonicalize(swift)
            .ok()
            .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
            .unwrap_or_else(|| swift.to_owned());
        let bin_dir = real.parent().map(Utf8Path::to_owned).unwrap_or_default();
        let text = version_output(runner, swift).await.map_err(|e| {
            Error::new(codes::toolchain::BROKEN, format!("`{swift} --version` failed: {}", e.message()))
                .with_hint("Check that `swift --version` runs (`russet doctor`); reinstall Swift 6.4 if it does not.")
        })?;
        let version_text = text.lines().find(|l| l.contains("Swift version")).unwrap_or_default().trim().to_owned();
        let version = parse_swift_version(&version_text);
        Ok(Self {
            swift: swift.to_owned(),
            version,
            version_text,
            llvm_strip: llvm_rewriter(runner, &bin_dir, "llvm-strip").await,
            llvm_lipo: llvm_rewriter(runner, &bin_dir, "llvm-lipo").await,
            llvm_otool: llvm_rewriter(runner, &bin_dir, "llvm-otool").await,
            llvm_install_name_tool: llvm_rewriter(runner, &bin_dir, "llvm-install-name-tool").await,
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

/// Probe output is parsed, never shown.
struct Quiet;

impl ProgressSink for Quiet {
    fn begin(&self, _title: &str, _cancellable: bool) {}
    fn report(&self, _msg: Option<&str>, _pct: Option<u8>) {}
    fn end(&self, _msg: Option<&str>) {}
    fn log(&self, _stream: LogStream, _line: &str) {}
}

impl EventSink for Quiet {
    fn emit(&self, _e: JobEventKind) {}
}

/// The stdout of `<tool> --version` (exit 0 within [`PROBE_TIMEOUT`]).
async fn version_output(runner: &dyn CommandRunner, tool: &Utf8Path) -> Result<String> {
    let spec = CommandSpec {
        program: tool.as_std_path().to_owned(),
        args: vec!["--version".into()],
        timeout: Some(PROBE_TIMEOUT),
        ..CommandSpec::default()
    };
    let out = runner.run(&spec, &Quiet, &CancellationToken::new()).await?;
    if out.code != Some(0) {
        let why = out.signal.map_or_else(|| format!("exit {:?}", out.code), |s| format!("signal {s}"));
        return Err(Error::new(codes::toolchain::BROKEN, format!("`{tool} --version` ended with {why}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `name` from `bin_dir`, else from `PATH`, when it is LLVM ≥ [`MIN_LLVM_REWRITER`].
async fn llvm_rewriter(runner: &dyn CommandRunner, bin_dir: &Utf8Path, name: &str) -> Option<Utf8PathBuf> {
    let candidate = bin_dir.join(exe(name));
    let tool = if candidate.is_file() { candidate } else { which(name)? };
    let text = version_output(runner, &tool).await.ok()?;
    (llvm_major(&text)? >= MIN_LLVM_REWRITER).then_some(tool)
}

/// `Swift version 6.4 (swift-6.4-RELEASE)` → 6.4; also accepts `Apple Swift version 6.4.1 …`.
pub fn parse_swift_version(line: &str) -> Option<Version> {
    let after = line.split("Swift version").nth(1)?;
    Version::parse(after.split_whitespace().next()?)
}

/// The major version in `<tool> --version` output (`LLVM version 23.1.1`).
fn llvm_major(text: &str) -> Option<u32> {
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

    #[test]
    fn llvm_versions_parse() {
        assert_eq!(llvm_major("LLVM (http://llvm.org/):\n  LLVM version 23.1.1\n  Optimized build."), Some(23));
        assert_eq!(llvm_major("Homebrew LLVM version 21.1.6"), Some(21));
        assert_eq!(llvm_major("GNU strip 2.44"), None);
    }
}
