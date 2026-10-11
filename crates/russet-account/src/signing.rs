// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `sign.run` and the pipeline's signing step: choose the identity and a profile per app-like bundle (explicit,
//! or from the profile cache), then sign through russet-sign on a blocking thread.

use std::collections::BTreeMap;
use std::path::PathBuf;

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_sign::{
    AppBundle, BundleSigningInput, Distribution, LintProfile, MergePolicy, PreSignFixes, ProvisioningProfile,
    SignOptions, SignOutput, SignReport, SignRequest,
};

use crate::AccountService;
use crate::identity::{IdentityId, SigningIdentity};
use crate::profiles::{ProfileQuery, kind_for};

/// What to sign and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignAppRequest {
    /// The `.app` to sign.
    pub path: PathBuf,
    pub distribution: Distribution,
    /// A specific identity; otherwise the newest identity every chosen profile lists.
    pub identity: Option<IdentityId>,
    /// Explicit profiles by bundle path (`""` = the app); the others come from the profile cache.
    pub profiles: BTreeMap<String, PathBuf>,
    /// Requested entitlements by bundle path.
    pub entitlements: BTreeMap<String, PathBuf>,
    /// `get-task-allow` for every app-like bundle (debug builds); `None` takes each profile's value.
    pub get_task_allow: Option<bool>,
    pub merge_policy: MergePolicy,
    /// Sign a copy here; `None` signs in place.
    pub output: Option<PathBuf>,
    /// The device the profiles must list, when known.
    pub device: Option<String>,
    pub lint: Option<LintProfile>,
    pub fixes: PreSignFixes,
}

impl SignAppRequest {
    /// Defaults: development, cache profiles, least privilege, device-install lints, default fixes.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            distribution: Distribution::Development,
            identity: None,
            profiles: BTreeMap::new(),
            entitlements: BTreeMap::new(),
            get_task_allow: None,
            merge_policy: MergePolicy::Least,
            output: None,
            device: None,
            lint: Some(LintProfile::DeviceInstall),
            fixes: PreSignFixes::default(),
        }
    }
}

impl AccountService {
    /// Signs an app with a stored identity and the right profiles.
    pub async fn sign_app(&self, r: SignAppRequest) -> Result<SignReport> {
        if !r.path.is_dir() {
            return Err(Error::new(
                codes::sign::BUNDLE_INVALID,
                format!("{} is not an .app directory", r.path.display()),
            )
            .with_hint("Sign the .app the build produced; unpack an .ipa first (`russet sign` writes .app bundles)."));
        }
        let app = AppBundle::open(&r.path)?;
        let bundles: Vec<(String, String)> = app
            .all()
            .into_iter()
            .filter(|b| b.kind.needs_profile())
            .map(|b| (b.rel_path.clone(), b.identifier().unwrap_or_default().to_owned()))
            .collect();
        let explicit: BTreeMap<String, ProvisioningProfile> = r
            .profiles
            .iter()
            .map(|(rel, p)| Ok((rel.clone(), ProvisioningProfile::open(p)?)))
            .collect::<Result<_>>()?;

        let want_kind = kind_for(r.distribution);
        let candidates: Vec<SigningIdentity> = match &r.identity {
            Some(id) => vec![self.identities().get(id)?],
            None => self.identities().list(None)?.into_iter().filter(|i| !i.is_expired()).collect(),
        };
        if candidates.is_empty() {
            return Err(Error::new(codes::sign::NO_IDENTITY, "no signing identity is available")
                .with_hint("Import one: `russet account import --from-xtool --yes` (or a .p12 with W12)."));
        }
        // The first identity for which every app-like bundle has a profile.
        let mut chosen: Option<(SigningIdentity, BTreeMap<String, PathBuf>)> = None;
        let mut missing = String::new();
        'identities: for identity in candidates {
            let mut picks = BTreeMap::new();
            for (rel, bundle_id) in &bundles {
                if let (Some(p), Some(path)) = (explicit.get(rel), r.profiles.get(rel)) {
                    if p.has_certificate(&identity.cert_der) && p.covers(bundle_id, &identity.team_id) {
                        picks.insert(rel.clone(), path.clone());
                        continue;
                    }
                    missing =
                        format!("the profile given for {} does not list this identity's certificate", display(rel));
                    continue 'identities;
                }
                let q = ProfileQuery {
                    bundle_id,
                    team: &identity.team_id,
                    cert_der: &identity.cert_der,
                    distribution: r.distribution,
                    device: r.device.as_deref(),
                };
                match self.profiles().select(&q)? {
                    Some((p, path)) if p.kind == want_kind => {
                        picks.insert(rel.clone(), path);
                    }
                    _ => {
                        missing = format!("no cached {} profile covers {bundle_id}", r.distribution.as_str());
                        continue 'identities;
                    }
                }
            }
            chosen = Some((identity, picks));
            break;
        }
        let Some((identity, picks)) = chosen else {
            return Err(Error::new(codes::sign::PROFILE_MISMATCH, format!("cannot sign {}: {missing}", r.path.display()))
                .with_hint("Run `russet provision` for the project (it caches the profiles), or pass --profile <bundle-rel>=<file>."));
        };

        let per_bundle: BTreeMap<String, BundleSigningInput> = bundles
            .iter()
            .map(|(rel, _)| {
                (
                    rel.clone(),
                    BundleSigningInput {
                        profile: picks.get(rel).cloned(),
                        entitlements: r.entitlements.get(rel).cloned(),
                        bundle_id_override: None,
                        get_task_allow: r.get_task_allow,
                    },
                )
            })
            .collect();
        let loaded = self.identities().load(&identity.id).await?.into_inner();
        let options = SignOptions {
            output: r.output.clone().map_or(SignOutput::InPlace, SignOutput::CopyTo),
            distribution: r.distribution,
            merge_policy: r.merge_policy,
            fixes: r.fixes,
            lint: r.lint,
        };
        let path = r.path.clone();
        tokio::task::spawn_blocking(move || {
            russet_sign::sign(SignRequest { bundle: &path, identity: &loaded, per_bundle, options })
        })
        .await
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("the signing task failed: {e}")))?
    }
}

fn display(rel: &str) -> &str {
    if rel.is_empty() { "the app" } else { rel }
}
