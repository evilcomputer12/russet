// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Mach-O parsing and in-place edits (ARCHITECTURE §5.3 `russet-macho`, the union of build §4.7 and SIGN §8.5)
//! [D 20]: thin and fat files, `LC_BUILD_VERSION`, rpaths, load commands, UUID, the iOS 27 `LC_SYMTAB`
//! string-pool alignment rule, code-signature blobs, and a last-resort SDK patch for foreign binaries.
//!
//! Layer F (MIT). F0 stub (W1); W6 fills the bodies (SA-03).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::path::Path;

pub use uuid::Uuid;

/// Errors of this crate (layer F has no dependency on `russet-core`).
#[derive(Debug, thiserror::Error)]
pub enum MachOError {
    #[error("malformed Mach-O: {0}")]
    Malformed(String),
    #[error("unsupported Mach-O: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A version as Mach-O encodes it (`xxxx.yy.zz` in `LC_BUILD_VERSION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version {
    pub major: u16,
    pub minor: u8,
    pub patch: u8,
}

/// CPU architecture of a slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    Arm64,
    Arm64e,
    X86_64,
}

/// One architecture slice of a thin or fat file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slice {
    pub arch: Arch,
    pub offset: u64,
    pub size: u64,
}

/// `LC_BUILD_VERSION`: platform, minimum OS, SDK and tool versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildVersion {
    pub platform: u32,
    pub minos: Version,
    pub sdk: Version,
    pub tools: Vec<(u32, Version)>,
}

/// A dylib load command (`LC_LOAD_DYLIB` and friends).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DylibRef {
    pub name: String,
    pub weak: bool,
}

/// `LC_ENCRYPTION_INFO_64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncInfo {
    pub cryptoff: u32,
    pub cryptsize: u32,
    pub cryptid: u32,
}

/// The parsed code-signature superblob: code directories, entitlements, team. W6 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSignature {
    _f0: (),
}

/// A parsed thin or fat Mach-O view.
#[derive(Debug)]
pub struct MachO {
    _f0: (),
}

impl MachO {
    pub fn parse(bytes: &[u8]) -> Result<Self, MachOError> {
        todo!("W6 (SA-03)")
    }
    pub fn slices(&self) -> &[Slice] {
        todo!("W6 (SA-03)")
    }
    pub fn build_version(&self) -> Option<BuildVersion> {
        todo!("W6 (SA-03)")
    }
    pub fn uuid(&self) -> Option<Uuid> {
        todo!("W6 (SA-03)")
    }
    pub fn rpaths(&self) -> Vec<String> {
        todo!("W6 (SA-03)")
    }
    pub fn dylibs(&self) -> Vec<DylibRef> {
        todo!("W6 (SA-03)")
    }
    pub fn encryption_info(&self) -> Option<EncInfo> {
        todo!("W6 (SA-03)")
    }
    /// Alignment of the `LC_SYMTAB` string pool (iOS 27 requires 8).
    pub fn string_pool_alignment(&self) -> u32 {
        todo!("W6 (SA-03)")
    }
    pub fn code_signature(&self) -> Option<CodeSignature> {
        todo!("W6 (SA-03)")
    }
}

/// Deletes one `LC_RPATH` in place.
pub fn delete_rpath(path: &Path, rpath: &str) -> Result<(), MachOError> {
    todo!("W6 (SA-03)")
}

/// Fixes the iOS 27 8-byte string-pool rule in place; returns whether anything changed.
pub fn align_string_pool(path: &Path) -> Result<bool, MachOError> {
    todo!("W6 (SA-03)")
}

/// Thins a fat file to one architecture in place.
pub fn thin(path: &Path, arch: Arch) -> Result<(), MachOError> {
    todo!("W6 (SA-03)")
}

/// Rewrites the SDK version of a foreign binary. Never the default (ARCHITECTURE §16.4).
pub fn patch_sdk_version(path: &Path, sdk: Version) -> Result<(), MachOError> {
    todo!("W6 (SA-03)")
}
