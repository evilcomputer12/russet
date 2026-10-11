// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Code signing (ARCHITECTURE §5.3 `russet-sign`; SIGN §8): the bundle walk, provisioning profiles, the
//! entitlement merge, signing through apple-codesign 0.29.0, post-sign verification, and the L-SIG, L-MACHO and
//! L-BND lints.
//!
//! [`sign`] copies the app (or signs in place), applies the pre-sign fixes (exec bits, the iOS 27 string-pool
//! alignment, stale signature files), embeds each app-like bundle's profile, resolves its entitlements against
//! the profile, signs inside-out (frameworks and dylibs, then extensions, then the app) and verifies the result.
//! [`verify`], [`lint`] and [`inspect`] read `.app` directories and `.ipa` files.
//!
//! The `test-pki` feature adds [`testpki`]: a synthetic CA, signing identities and CMS-signed profiles for tests
//! (SIGN §18; no Apple key material).

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use russet_core::Result;
/// apple-codesign 0.29.0's signing-key trait. It lives in `x509-certificate` 0.24, which apple-codesign uses but
/// does not re-export, so ARCHITECTURE §5.3's `apple_codesign::KeyInfoSigner` is spelled through this re-export
/// [D 92].
pub use x509_certificate::KeyInfoSigner;

mod bundle;
mod entitlements;
mod fixes;
mod identity;
mod inspect;
mod ipa;
mod lint;
mod profile;
mod signer;
#[cfg(feature = "test-pki")]
pub mod testpki;
mod util;
mod verify;

pub use bundle::{AppBundle, BundleKind, CodeItem};
pub use entitlements::{
    CapabilityInfo, Distribution, EntitlementsError, EntitlementsResolver, MergeNote, MergePolicy,
    ResolvedEntitlements, allowed_value, capability_for_entitlement,
};
pub use fixes::{FixRecord, PreSignFixes};
pub use identity::{CertificateInfo, CertificateKind, InMemoryIdentity, certificate_info};
pub use inspect::{CodeDirectoryInfo, SignatureInfo};
pub use ipa::OpenedInput;
pub use profile::{ProfileKind, ProvisioningProfile};
pub use util::{glob_match, hex, is_team_like, mask_team, normalize_serial, redact_app_id, redact_error, redact_teams};
pub use verify::{VerifiedBundle, VerifyOptions, VerifyReport};

/// A signing identity; implemented by russet-account's `LoadedIdentity`, which breaks the sign ↔ account cycle,
/// and by [`InMemoryIdentity`].
pub trait CodeSigningIdentity: Send + Sync {
    /// The DER certificate.
    fn certificate_der(&self) -> &[u8];
    /// The team the certificate belongs to (its subject OU).
    fn team_id(&self) -> &str;
    /// The private key, as apple-codesign signs with it.
    fn signer(&self) -> &dyn KeyInfoSigner;
}

/// Per-bundle inputs, keyed in [`SignRequest::per_bundle`] by the bundle's path relative to the app (`""` is the
/// app, `PlugIns/Widget.appex` an extension). The two fields are ARCHITECTURE §5.3's (F0); the per-bundle
/// bundle-ID rewrite and `get-task-allow` live in [`SignOptions`], so callers can keep building this struct
/// with these two fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BundleSigningInput {
    /// The `.mobileprovision` to embed; required for app-like bundles (apps, extensions, App Clips).
    pub profile: Option<PathBuf>,
    /// The requested entitlements (an `.entitlements` plist); `None` signs only the keys the merge requires.
    pub entitlements: Option<PathBuf>,
}

impl BundleSigningInput {
    /// A bundle signed with `profile` and no requested entitlements.
    pub fn with_profile(profile: PathBuf) -> Self {
        Self { profile: Some(profile), entitlements: None }
    }
}

/// Where the signed bundle goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SignOutput {
    /// Sign the bundle in place (the default; SIGN §8.1 prefers a copy, see the Wave-1 notes in SIGN §23).
    #[default]
    InPlace,
    /// Copy the bundle to this path (an `.app` path) and sign the copy; the input stays unsigned (ARCHITECTURE
    /// §14.8: `.russet/build/<profile>-iphoneos/signed/App.app`). An existing destination is replaced only when it
    /// is an earlier signed output of the same app: an `.app` with `_CodeSignature/CodeResources` and the same
    /// `CFBundleIdentifier`. A destination inside the input, or containing it, is refused.
    CopyTo(PathBuf),
}

/// Signing options (SIGN §8.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignOptions {
    pub output: SignOutput,
    pub distribution: Distribution,
    pub merge_policy: MergePolicy,
    pub fixes: PreSignFixes,
    /// Lints to run after signing; their errors fail the signing (`sign.lintFailed`). `None` skips them.
    pub lint: Option<LintProfile>,
    /// Requests `get-task-allow` for every app-like bundle (the build plane's `SignItem::get_task_allow` of a
    /// debug build); `None` takes each profile's value. A key in a bundle's entitlements file wins.
    pub get_task_allow: Option<bool>,
    /// Rewrites `CFBundleIdentifier` before signing, by bundle path (`""` = the app): re-signing an app onto
    /// another App ID.
    pub bundle_ids: BTreeMap<String, String>,
}

impl Default for SignOptions {
    fn default() -> Self {
        Self {
            output: SignOutput::InPlace,
            distribution: Distribution::Development,
            merge_policy: MergePolicy::Least,
            fixes: PreSignFixes::default(),
            lint: Some(LintProfile::DeviceInstall),
            get_task_allow: None,
            bundle_ids: BTreeMap::new(),
        }
    }
}

/// A signing request.
pub struct SignRequest<'a> {
    /// The `.app` to sign.
    pub bundle: &'a Path,
    pub identity: &'a dyn CodeSigningIdentity,
    /// Keyed by the path relative to the bundle; `""` is the app itself.
    pub per_bundle: BTreeMap<String, BundleSigningInput>,
    pub options: SignOptions,
}

/// One signed app-like bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedBundle {
    pub rel_path: String,
    pub bundle_id: String,
    pub kind: BundleKind,
    /// The CDHash (20 bytes, lower-case hex) of the strongest code directory of the main executable: what the
    /// install ledger records (ARCHITECTURE §14.8).
    pub cdhash: String,
    pub team_id: String,
    pub profile_uuid: String,
    /// The signed entitlements, as an XML plist.
    pub entitlements_xml: String,
    pub notes: Vec<MergeNote>,
}

/// What [`sign`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignReport {
    /// The signed bundle.
    pub output: PathBuf,
    /// App-like bundles, deepest first, the app last.
    pub bundles: Vec<SignedBundle>,
    pub fixes_applied: Vec<FixRecord>,
    pub lints: Vec<LintFinding>,
}

/// Which lint profile applies (SIGN §8.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LintProfile {
    DeviceInstall,
    AppStore,
}

impl LintProfile {
    /// `device-install` or `app-store`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeviceInstall => "device-install",
            Self::AppStore => "app-store",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "device-install" => Some(Self::DeviceInstall),
            "app-store" => Some(Self::AppStore),
            _ => None,
        }
    }
}

/// How bad a finding is: errors block signing and uploads; warnings do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// A signing lint finding (L-SIG, L-MACHO, L-BND; SIGN §8.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintFinding {
    /// For example `L-SIG-001`.
    pub code: String,
    pub message: String,
    pub severity: Severity,
    /// The file or bundle it concerns, relative to the checked bundle.
    pub path: Option<String>,
}

impl LintFinding {
    pub(crate) fn new(code: &str, severity: Severity, path: Option<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), severity, path }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Pre-fixes (chmod 0755, string-pool alignment), signs inside-out, then verifies.
pub fn sign(r: SignRequest<'_>) -> Result<SignReport> {
    signer::sign(r)
}

/// Verifies every signature of an `.app` or `.ipa`: code pages, CMS, sealed resources, the embedded profiles
/// and the L-SIG rules.
pub fn verify(path: &Path, o: &VerifyOptions) -> Result<VerifyReport> {
    verify::verify(path, o)
}

/// Runs the L-SIG, L-MACHO and L-BND lints of profile `p` on an `.app` or `.ipa`.
pub fn lint(path: &Path, p: LintProfile) -> Result<Vec<LintFinding>> {
    let input = ipa::open_input(path)?;
    let app = AppBundle::open(input.app())?;
    lint::lint(&app, p, input.is_ipa().then_some(input.ipa_root()).flatten())
}

/// Describes every signature of an `.app`, `.ipa` or single Mach-O file (team IDs masked).
pub fn inspect(path: &Path) -> Result<Vec<SignatureInfo>> {
    inspect::inspect(path)
}

/// Signing doctor checks, for russetd's doctor seam. Signing needs nothing from the environment: the signer is
/// pure Rust, and the identities and profiles are checked by russet-account's checks.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
