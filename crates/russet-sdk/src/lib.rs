// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! SDK facts, the platform view, toolsets, and SDK and DDI import (ARCHITECTURE §5.3 `russet-sdk`, §16.3;
//! build §3.4, §9.1) [D 58, D 62, D 69].
//!
//! - [`facts`] reads an SDK bundle in place: `SDKSettings`, `SystemVersion.plist`, the platform `Info.plist`
//!   (`AdditionalInfo`), the platform and Xcode `version.plist`s, the back-deployment dylib directories and the
//!   Swift version the SDK's interfaces were built with.
//! - [`view`] materializes the Russet-owned platform view (`data/sdk/<id>/view/Developer/…`) that
//!   `XCODE_EXTRA_PLATFORM_FOLDERS` points at; [`toolset`] writes `toolset-swb.json` and `toolset-native.json`
//!   with the link-time SDK fix (`-Xclang-linker -isysroot -Xclang-linker <SDK>`, §16.4).
//! - [`XtoolBundleProvider`] reads xtool's `~/.swiftpm/swift-sdks/*.artifactbundle` in place and never copies it;
//!   [`ImportedBundleProvider`] and [`import`] are W16b's.
//! - [`host::HostToolchain`] is the Swift toolchain the build runs with (W3's resolver supplies it in the daemon;
//!   [`host::HostToolchain::probe`] finds `swift` on `PATH` otherwise).
//!
//! W5 owns the crate (Wave 1); W16b owns [`import`], W27b `import::windows`.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use camino::Utf8PathBuf;
use russet_core::Result;
use russet_core::error::{Error, codes};

pub mod facts;
pub mod host;
// F0 stub (W16b fills it): its `todo!()` body leaves the parameters unused.
#[allow(unused_variables)]
pub mod import;
pub mod toolset;
pub mod view;
mod xtool;

pub use xtool::XtoolBundleProvider;

/// An Apple-style version (`27.0`, `6.4`, `17.0`, `27.0.99`); not semver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// `major.minor.patch`.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self { major, minor, patch }
    }

    /// Parses `27`, `27.0` or `27.0.1`; extra components and non-numeric text are refused.
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.trim().split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self { major, minor, patch })
    }

    /// The same major and minor version (the Swift ↔ SDK lock compares only these, §16.1).
    pub fn same_minor(self, other: Version) -> bool {
        self.major == other.major && self.minor == other.minor
    }

    /// Xcode's `XCODE_VERSION_ACTUAL` form, `MMmp`: 27.0 → `2700`, 9.4.1 → `0941`.
    pub fn xcode_actual(self) -> String {
        format!("{:02}{}{}", self.major, self.minor % 10, self.patch % 10)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.patch == 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        }
    }
}

impl FromStr for Version {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s).ok_or_else(|| Error::new(codes::core::INVALID_ARGUMENT, format!("`{s}` is not a version")))
    }
}

/// Which SDK to resolve: `auto`, a canonical name (`iphoneos27.0`) or an Xcode build (`27A266a`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SdkSelector {
    #[default]
    Auto,
    CanonicalName(String),
    XcodeBuild(String),
}

impl SdkSelector {
    /// Parses the `[build] sdk` value: `auto`, `iphoneos27.0` or an Xcode build such as `27A266a`.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.is_empty() || s == "auto" {
            Self::Auto
        } else if s.starts_with("iphoneos") {
            Self::CanonicalName(s.to_owned())
        } else {
            Self::XcodeBuild(s.to_owned())
        }
    }

    /// Whether `facts` satisfies the selector.
    pub fn matches(&self, facts: &SdkFacts) -> bool {
        match self {
            Self::Auto => true,
            Self::CanonicalName(n) => facts.sdk_canonical_name == *n,
            Self::XcodeBuild(b) => facts.xcode_build.eq_ignore_ascii_case(b),
        }
    }
}

impl fmt::Display for SdkSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::CanonicalName(n) => f.write_str(n),
            Self::XcodeBuild(b) => f.write_str(b),
        }
    }
}

/// Tools a build runs: the host Swift toolchain plus the SDK's linker, librarian and `dsymutil`, and the LLVM
/// rewriters (which must be LLVM ≥ 23, build §3.4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolPaths {
    /// The `swift` driver (`swift build`, `swift package`).
    pub swift: Utf8PathBuf,
    pub swiftc: Utf8PathBuf,
    pub clang: Option<Utf8PathBuf>,
    /// The host Swift version, for the SDK lock check (§16.1).
    pub swift_version: Option<Version>,
    /// The first line of `swift --version`, for build reports.
    pub swift_version_text: String,
    /// The directory the toolsets' `rootPath` names (the linker and the librarian live there).
    pub linker_dir: Utf8PathBuf,
    /// `ld64.lld` (darwin-tools, or LLVM ≥ 23.1.3).
    pub ld64_lld: Utf8PathBuf,
    /// `llvm-libtool-darwin`, named `llvm-lib` in SDK bundles (build §3.4).
    pub librarian: Utf8PathBuf,
    pub dsymutil: Option<Utf8PathBuf>,
    pub llvm_strip: Option<Utf8PathBuf>,
    pub llvm_lipo: Option<Utf8PathBuf>,
    pub llvm_otool: Option<Utf8PathBuf>,
    pub llvm_install_name_tool: Option<Utf8PathBuf>,
}

/// Everything the build plane needs to know about one SDK (build §3.4, §9.1).
#[derive(Debug, Clone, PartialEq)]
pub struct SdkFacts {
    /// `xcode-<Xcode version>-<Xcode build>`, the id `toolchain.list` and `sdk.list` report.
    pub id: String,
    /// `…/iPhoneOS27.0.sdk`.
    pub sdk_path: Utf8PathBuf,
    pub sdk_version: Version,
    /// `iphoneos27.0`.
    pub sdk_canonical_name: String,
    /// `SystemVersion.plist` `ProductBuildVersion` (`DTSDKBuild`).
    pub sdk_build: Option<String>,
    /// Platform `version.plist` `ProductBuildVersion` (`DTPlatformBuild`).
    pub platform_build: Option<String>,
    pub platform_version: String,
    pub xcode_version: Version,
    pub xcode_build: String,
    /// The Swift version the SDK needs (Xcode 27 → 6.4).
    pub swift_required: Version,
    pub valid_deployment_targets: Vec<Version>,
    /// The platform `Info.plist` `AdditionalInfo` template (the DT* keys).
    pub additional_info: plist::Dictionary,
    /// `…/usr/lib/swift-{5.0,5.5,6.2}/iphoneos`.
    pub back_deploy_dirs: Vec<Utf8PathBuf>,
    /// The value for `XCODE_EXTRA_PLATFORM_FOLDERS`: `data/sdk/<id>/view/Developer/Platforms`.
    pub platforms_view: Utf8PathBuf,
    pub toolset_swb: Utf8PathBuf,
    pub toolset_native: Utf8PathBuf,
    pub tools: ToolPaths,
    /// The Swift SDK bundle root (an `.artifactbundle` or a Russet import).
    pub bundle: Utf8PathBuf,
    /// `DEFAULT_COMPILER` (`DTCompiler`'s source; only `xcode-compat` writes it).
    pub default_compiler: Option<String>,
    /// `SwiftCompatibilityLibraryNotNeededStartingIn`: back-deployment dylib → first OS that ships it.
    pub back_deploy_not_needed_from: BTreeMap<String, Version>,
    /// The bundle's darwin-tools version, when it has one (`darwin-sdk-version.txt`).
    pub darwin_tools: Option<String>,
}

impl SdkFacts {
    /// Whether `target` is one of the SDK's `ValidDeploymentTargets`.
    pub fn accepts_deployment_target(&self, target: Version) -> bool {
        self.valid_deployment_targets.contains(&target)
    }
}

/// Resolves an SDK to its facts.
#[async_trait]
pub trait SdkProvider: Send + Sync {
    async fn resolve(&self, sel: &SdkSelector) -> Result<SdkFacts>;
}

/// Russet-imported bundles in `data/sdk/<id>/` (Linux) or the Windows short root. W16b fills it.
#[derive(Debug)]
pub struct ImportedBundleProvider {
    _f0: (),
}

#[async_trait]
impl SdkProvider for ImportedBundleProvider {
    async fn resolve(&self, _sel: &SdkSelector) -> Result<SdkFacts> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "Russet-imported SDK bundles land with W16b")
            .with_hint("Use xtool's SDK bundle for now (`xtool setup`)."))
    }
}

/// SDK doctor checks, for russetd's doctor seam: `sdk.buildFacts` (an SDK the build plane can use).
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    vec![Arc::new(xtool::BuildFactsCheck)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_print_and_compare() {
        assert_eq!(Version::parse("27.0"), Some(Version::new(27, 0, 0)));
        assert_eq!(Version::parse("27.0.99"), Some(Version::new(27, 0, 99)));
        assert_eq!(Version::parse("17"), Some(Version::new(17, 0, 0)));
        assert_eq!(Version::parse("6.4.1.2"), None);
        assert_eq!(Version::parse("x"), None);
        assert_eq!(Version::new(27, 0, 0).to_string(), "27.0");
        assert_eq!(Version::new(26, 4, 1).to_string(), "26.4.1");
        assert!(Version::new(17, 0, 0) < Version::new(17, 6, 0));
        assert!(Version::new(6, 4, 0).same_minor(Version::new(6, 4, 2)));
        assert_eq!(Version::new(27, 0, 0).xcode_actual(), "2700");
        assert_eq!(Version::new(9, 4, 1).xcode_actual(), "0941");
        assert!("abc".parse::<Version>().is_err());
    }

    #[test]
    fn selectors_parse_from_the_build_section() {
        assert_eq!(SdkSelector::parse("auto"), SdkSelector::Auto);
        assert_eq!(SdkSelector::parse(""), SdkSelector::Auto);
        assert_eq!(SdkSelector::parse("iphoneos27.0"), SdkSelector::CanonicalName("iphoneos27.0".into()));
        assert_eq!(SdkSelector::parse("27A266a"), SdkSelector::XcodeBuild("27A266a".into()));
        assert_eq!(SdkSelector::parse("27A266a").to_string(), "27A266a");
    }
}
