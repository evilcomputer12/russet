// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Code signing (ARCHITECTURE §5.3 `russet-sign`; SIGN §8.7): bundle walk, provisioning profiles, entitlement
//! merge, signing through apple-codesign, verification and lints.
//!
//! F0 stub (W1); W6 fills the bodies (SA-04, SA-05).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use russet_core::Result;
/// apple-codesign 0.29.0's signing-key trait. It lives in `x509-certificate` 0.24, which apple-codesign uses but
/// does not re-export, so ARCHITECTURE §5.3's `apple_codesign::KeyInfoSigner` is spelled through this re-export.
pub use x509_certificate::KeyInfoSigner;

/// A signing identity; implemented by russet-account's `LoadedIdentity`, which breaks the sign ↔ account cycle.
pub trait CodeSigningIdentity: Send + Sync {
    fn certificate_der(&self) -> &[u8];
    fn team_id(&self) -> &str;
    fn signer(&self) -> &dyn KeyInfoSigner;
}

/// A parsed provisioning profile (the CMS payload).
#[derive(Debug, Clone)]
pub struct ProvisioningProfile {
    _f0: (),
}

impl ProvisioningProfile {
    pub fn parse(der: &[u8]) -> Result<Self> {
        todo!("W6 (SA-04)")
    }
    pub fn covers(&self, bundle_id: &str, team: &str) -> bool {
        todo!("W6 (SA-04)")
    }
    pub fn has_certificate(&self, der: &[u8]) -> bool {
        todo!("W6 (SA-04)")
    }
    pub fn has_device(&self, udid: &str) -> bool {
        todo!("W6 (SA-04)")
    }
    pub fn expires_within(&self, d: Duration) -> bool {
        todo!("W6 (SA-04)")
    }
}

/// Per-bundle inputs: the profile and entitlements of one nested bundle. W6 defines the remaining fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BundleSigningInput {
    pub profile: Option<PathBuf>,
    pub entitlements: Option<PathBuf>,
}

/// Signing options. W6 defines the fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignOptions {}

/// A signing request.
pub struct SignRequest<'a> {
    pub bundle: &'a Path,
    pub identity: &'a dyn CodeSigningIdentity,
    /// Keyed by the path relative to the bundle; `""` is the app itself.
    pub per_bundle: BTreeMap<String, BundleSigningInput>,
    pub options: SignOptions,
}

/// What [`sign`] did. W6 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignReport {
    _f0: (),
}

/// Verification options. W6 defines the fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyOptions {}

/// What [`verify`] found. W6 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    _f0: (),
}

/// Which lint profile applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintProfile {
    DeviceInstall,
    AppStore,
}

/// A signing lint finding (L-SIG, L-MACHO, L-BND).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintFinding {
    pub code: String,
    pub message: String,
}

/// One signature found by [`inspect`]. W6 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    _f0: (),
}

/// Pre-fixes (chmod 0755, string-pool alignment), signs inside-out, then verifies.
pub fn sign(r: SignRequest<'_>) -> Result<SignReport> {
    todo!("W6 (SA-05)")
}

pub fn verify(path: &Path, o: &VerifyOptions) -> Result<VerifyReport> {
    todo!("W6 (SA-05)")
}

pub fn lint(path: &Path, p: LintProfile) -> Result<Vec<LintFinding>> {
    todo!("W6 (SA-05)")
}

pub fn inspect(path: &Path) -> Result<Vec<SignatureInfo>> {
    todo!("W6 (SA-05)")
}

/// Signing doctor checks, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
