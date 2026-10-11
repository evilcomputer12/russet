// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! SDK facts, the platform view, toolsets, and SDK and DDI import (ARCHITECTURE §5.3 `russet-sdk`, §16.3;
//! build §9.1) [D 58, D 62, D 69].
//!
//! F0 stub (W1); W5 fills it (B03 and the SDK facts), W16b owns [`import`], W27b `import::windows`.

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

use std::sync::Arc;

use async_trait::async_trait;
use camino::Utf8PathBuf;
use russet_core::Result;

pub mod import;

/// An Apple-style version (`27.0`, `6.4`, `17.0`); not semver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// Which SDK to resolve: `auto`, a canonical name (`iphoneos27.0`) or an Xcode build (`27A266a`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SdkSelector {
    #[default]
    Auto,
    CanonicalName(String),
    XcodeBuild(String),
}

/// Tool paths inside the SDK bundle: swift, swiftc, clang, ld64.lld, libtool, dsymutil, llvm-{strip,lipo,otool,
/// install-name-tool}. W5 defines the fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolPaths {}

/// Everything the build plane needs to know about one SDK (build §9.1).
#[derive(Debug, Clone, PartialEq)]
pub struct SdkFacts {
    pub id: String,
    pub sdk_path: Utf8PathBuf,
    pub sdk_version: Version,
    pub sdk_canonical_name: String,
    pub sdk_build: Option<String>,
    pub platform_build: Option<String>,
    pub platform_version: String,
    pub xcode_version: Version,
    pub xcode_build: String,
    pub swift_required: Version,
    pub valid_deployment_targets: Vec<Version>,
    pub additional_info: plist::Dictionary,
    pub back_deploy_dirs: Vec<Utf8PathBuf>,
    pub platforms_view: Utf8PathBuf,
    pub toolset_swb: Utf8PathBuf,
    pub toolset_native: Utf8PathBuf,
    pub tools: ToolPaths,
}

/// Resolves an SDK to its facts.
#[async_trait]
pub trait SdkProvider: Send + Sync {
    async fn resolve(&self, sel: &SdkSelector) -> Result<SdkFacts>;
}

/// Reads `~/.swiftpm/swift-sdks/*.artifactbundle` in place; never copies (Linux).
#[derive(Debug)]
pub struct XtoolBundleProvider {
    _f0: (),
}

#[async_trait]
impl SdkProvider for XtoolBundleProvider {
    async fn resolve(&self, sel: &SdkSelector) -> Result<SdkFacts> {
        todo!("W5 (B03)")
    }
}

/// Russet-imported bundles in `data/sdk/<id>/` (Linux) or the Windows short root.
#[derive(Debug)]
pub struct ImportedBundleProvider {
    _f0: (),
}

#[async_trait]
impl SdkProvider for ImportedBundleProvider {
    async fn resolve(&self, sel: &SdkSelector) -> Result<SdkFacts> {
        todo!("W16b")
    }
}

/// SDK doctor checks, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
