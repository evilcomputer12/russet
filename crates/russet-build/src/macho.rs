// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The Mach-O operations the build plane needs, behind one seam: summaries for the build report, `build.inspect`,
//! back-deployment and `BuiltApp.uuids`, and the in-place `LC_RPATH` deletion of rpath hygiene (build §4.6).
//!
//! [`RussetMachO`], the default, is `russet-macho` (W6, ARCHITECTURE §5.3 [D 20]): one shared parser and editor
//! for the build and signing planes. Tests substitute their own [`MachOTool`].

use camino::Utf8Path;
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_sdk::Version;
use uuid::Uuid;

/// `LC_BUILD_VERSION` platform number of iOS.
pub const PLATFORM_IOS: u32 = 2;

/// What the build plane reads from one Mach-O file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachOSummary {
    /// Architectures of the slices (`arm64`, `arm64e`, `x86_64`).
    pub archs: Vec<String>,
    /// `LC_BUILD_VERSION`: platform, minimum OS, SDK, and tools as `lld 21.1.6`.
    pub platform: Option<u32>,
    pub minos: Option<Version>,
    pub sdk: Option<Version>,
    pub tools: Vec<String>,
    pub uuid: Option<Uuid>,
    pub rpaths: Vec<String>,
    /// Install names of the dylibs it loads.
    pub dylibs: Vec<String>,
    pub encrypted: Option<bool>,
}

/// Mach-O reads and in-place edits.
pub trait MachOTool: Send + Sync {
    /// Parses a thin or fat Mach-O image.
    fn summary(&self, bytes: &[u8]) -> Result<MachOSummary>;
    /// Deletes one `LC_RPATH` in place.
    fn delete_rpath(&self, path: &Utf8Path, rpath: &str) -> Result<()>;
}

/// `russet-macho` (W6).
#[derive(Debug, Clone, Copy, Default)]
pub struct RussetMachO;

fn version(v: russet_macho::Version) -> Version {
    Version::new(u32::from(v.major), u32::from(v.minor), u32::from(v.patch))
}

/// The tool names of `LC_BUILD_VERSION` (`clang`, `swift`, `ld`, `lld`).
pub fn tool_name(tool: u32) -> String {
    match tool {
        1 => "clang".into(),
        2 => "swift".into(),
        3 => "ld".into(),
        4 => "lld".into(),
        other => format!("tool{other}"),
    }
}

fn macho_error(e: russet_macho::MachOError) -> Error {
    Error::new(codes::core::IO, format!("Mach-O: {e}"))
        .with_hint("The file is damaged or not a Mach-O image; rebuild it (`russet build --clean`).")
        .caused_by(e)
}

/// The name of a slice's architecture. W6's russet-macho adds `Arch::Other { cputype, cpusubtype }` (armv7,
/// arm64_32, …); the catch-all arm names it and keeps this match compiling against both the F0 enum and W6's.
fn arch_name(arch: russet_macho::Arch) -> String {
    match arch {
        russet_macho::Arch::Arm64 => "arm64".to_owned(),
        russet_macho::Arch::Arm64e => "arm64e".to_owned(),
        russet_macho::Arch::X86_64 => "x86_64".to_owned(),
        #[allow(unreachable_patterns)]
        other => format!("{other:?}"),
    }
}

impl MachOTool for RussetMachO {
    fn summary(&self, bytes: &[u8]) -> Result<MachOSummary> {
        let m = russet_macho::MachO::parse(bytes).map_err(macho_error)?;
        let archs = m.slices().iter().map(|s| arch_name(s.arch)).collect();
        let bv = m.build_version();
        Ok(MachOSummary {
            archs,
            platform: bv.as_ref().map(|b| b.platform),
            minos: bv.as_ref().map(|b| version(b.minos)),
            sdk: bv.as_ref().map(|b| version(b.sdk)),
            tools: bv
                .map(|b| b.tools.iter().map(|(t, v)| format!("{} {}", tool_name(*t), version(*v))).collect())
                .unwrap_or_default(),
            uuid: m.uuid(),
            rpaths: m.rpaths(),
            dylibs: m.dylibs().into_iter().map(|d| d.name).collect(),
            encrypted: m.encryption_info().map(|e| e.cryptid != 0),
        })
    }

    fn delete_rpath(&self, path: &Utf8Path, rpath: &str) -> Result<()> {
        russet_macho::delete_rpath(path.as_std_path(), rpath).map_err(macho_error)
    }
}

/// Whether an `LC_RPATH` names a host path that does not exist on iOS: absolute, and neither `/usr/lib/swift`,
/// `/usr/lib` nor under `/System/`. `@executable_path/…`, `@loader_path/…` and `/usr/lib/swift` stay [V build V18].
pub fn is_host_rpath(rpath: &str) -> bool {
    if rpath.starts_with('@') || !rpath.starts_with('/') {
        return false;
    }
    let trimmed = rpath.trim_end_matches('/');
    !(trimmed == "/usr/lib/swift" || trimmed == "/usr/lib" || trimmed.starts_with("/System/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_rpaths_are_recognized() {
        assert!(is_host_rpath("/home/u/p/.russet/work/swiftpm/out/Products/Debug-iphoneos/PackageFrameworks"));
        assert!(is_host_rpath("/usr/lib/swift/usr/lib/swift-6.2/iphoneos"));
        assert!(!is_host_rpath("/usr/lib/swift"));
        assert!(!is_host_rpath("/usr/lib/swift/"));
        assert!(!is_host_rpath("@executable_path/Frameworks"));
        assert!(!is_host_rpath("@loader_path"));
        assert!(!is_host_rpath("/System/Library/Frameworks"));
    }

    #[test]
    fn tool_names() {
        assert_eq!(tool_name(4), "lld");
        assert_eq!(tool_name(9), "tool9");
    }

    #[test]
    fn arch_names() {
        assert_eq!(arch_name(russet_macho::Arch::Arm64), "arm64");
        assert_eq!(arch_name(russet_macho::Arch::Arm64e), "arm64e");
        assert_eq!(arch_name(russet_macho::Arch::X86_64), "x86_64");
    }
}
