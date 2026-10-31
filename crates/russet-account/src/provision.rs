// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The provisioning engine (SIGN §6.4): `plan` reads the portal (GET-only) and lists the account changes a
//! request needs; `apply` performs a zero-mutation plan (it downloads and caches the chosen profiles) and
//! refuses mutations without an approval (W12 applies them). An approval binds to the plan's digest.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::ids::ApprovalId;
use russet_sign::{Distribution, EntitlementsResolver, MergePolicy, ProvisioningProfile, capability_for_entitlement};
use sha2::Digest as _;

use crate::fsutil::prefixed_ulid;
use crate::identity::{CertKind, IdentityId, IdentityStore, SigningIdentity};
use crate::portal::{BundleIdRecord, DevPortal};
use crate::profiles::{ProfileCache, kind_for};
use crate::store::AccountId;

/// A provisioning plan id (`pln_…`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlanId(pub String);

impl PlanId {
    pub fn generate() -> Self {
        Self(prefixed_ulid("pln_"))
    }
}

/// One bundle to provision: the app first, then extensions.
#[derive(Debug, Clone, PartialEq)]
pub struct BundleSpec {
    /// `""` for the app, `PlugIns/Widget.appex` for an extension.
    pub rel_path: String,
    /// The effective bundle ID (after the bundle-ID policy).
    pub bundle_id: String,
    /// The requested entitlements (the project's `.entitlements`).
    pub requested: plist::Dictionary,
}

impl BundleSpec {
    /// A bundle with the requested entitlements read from an `.entitlements` file (the build plane's
    /// `SignItem::entitlements`), or none. W7's pipeline builds its request from the build output this way.
    pub fn with_entitlements_file(
        rel_path: &str,
        bundle_id: &str,
        entitlements: Option<&std::path::Path>,
    ) -> Result<Self> {
        let requested = match entitlements {
            Some(p) => plist::Value::from_file(p)
                .map_err(|e| {
                    Error::new(codes::config::INVALID, format!("cannot read the entitlements {}: {e}", p.display()))
                        .with_hint("The .entitlements file must be an XML or binary property list.")
                })?
                .into_dictionary()
                .ok_or_else(|| {
                    Error::new(codes::config::INVALID, format!("{} is not a dictionary", p.display()))
                        .with_hint("An .entitlements file is a property list whose top level is a dictionary.")
                })?,
            None => plist::Dictionary::new(),
        };
        Ok(Self { rel_path: rel_path.to_owned(), bundle_id: bundle_id.to_owned(), requested })
    }
}

/// What to provision for (`provision.plan` params, resolved from the project).
#[derive(Debug, Clone, PartialEq)]
pub struct ProvisionRequest {
    pub bundles: Vec<BundleSpec>,
    pub distribution: Distribution,
    /// Devices the profile must list (development and ad hoc); empty = not checked.
    pub devices: Vec<String>,
    /// A specific identity, else the newest usable one.
    pub identity: Option<IdentityId>,
}

/// One Apple-side mutation of a plan (SIGN §6.4). W12 applies them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Mutation {
    CreateCertificate {
        kind: CertKind,
    },
    /// The UDID is redacted (first and last four characters).
    RegisterDevice {
        udid_redacted: String,
    },
    RegisterBundleId {
        identifier: String,
        name: String,
    },
    EnableCapability {
        bundle: String,
        capability: String,
    },
    CreateProfile {
        name: String,
        kind: String,
        bundle: String,
        devices: usize,
    },
    DeleteProfile {
        id: String,
        name: String,
    },
}

impl Mutation {
    /// A one-line description for plans and approvals.
    pub fn summary(&self) -> String {
        match self {
            Self::CreateCertificate { kind } => format!("create a {} certificate", kind.as_str()),
            Self::RegisterDevice { udid_redacted } => format!("register the device {udid_redacted}"),
            Self::RegisterBundleId { identifier, .. } => format!("register the bundle ID {identifier}"),
            Self::EnableCapability { bundle, capability } => format!("enable {capability} on {bundle}"),
            Self::CreateProfile { name, kind, .. } => format!("create the {kind} profile \"{name}\""),
            Self::DeleteProfile { name, .. } => format!("delete the Russet profile \"{name}\""),
        }
    }

    /// The kebab-case kind (`create-certificate`, …).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::CreateCertificate { .. } => "create-certificate",
            Self::RegisterDevice { .. } => "register-device",
            Self::RegisterBundleId { .. } => "register-bundle-id",
            Self::EnableCapability { .. } => "enable-capability",
            Self::CreateProfile { .. } => "create-profile",
            Self::DeleteProfile { .. } => "delete-profile",
        }
    }
}

/// A plan warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanWarning {
    pub message: String,
}

/// An existing profile the plan uses as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedProfile {
    pub rel_path: String,
    pub bundle_id: String,
    /// The portal's profile id.
    pub profile_id: String,
    pub uuid: String,
    pub name: String,
    pub expires: String,
    /// The profile bytes as read while planning.
    pub content: Vec<u8>,
}

/// A GET-only provisioning plan; `digest` binds approvals to it (ARCHITECTURE §11.3).
#[derive(Debug, Clone, PartialEq)]
pub struct ProvisionPlan {
    pub plan_id: PlanId,
    pub digest: [u8; 32],
    pub team_id: String,
    pub mutations: Vec<Mutation>,
    pub warnings: Vec<PlanWarning>,
    pub account_id: AccountId,
    pub distribution: Distribution,
    /// The identity that signs, when one is usable.
    pub identity: Option<IdentityId>,
    pub profiles: Vec<PlannedProfile>,
    /// What the plan was made for (`apply` re-plans it to detect staleness).
    pub request: ProvisionRequest,
}

/// The approval that authorizes a plan's mutations (none for a zero-mutation plan).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApprovalRef {
    pub approval_id: Option<ApprovalId>,
}

/// One provisioned bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleProvision {
    pub rel_path: String,
    pub bundle_id: String,
    pub profile_uuid: String,
    /// The cached profile (`data/profiles/<uuid>.mobileprovision`).
    pub profile_path: std::path::PathBuf,
}

/// What `provision.apply` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionOutcome {
    pub identity: Option<IdentityId>,
    pub bundles: Vec<BundleProvision>,
}

/// Plans and applies provisioning for one team.
pub struct Provisioner {
    portal: Arc<dyn DevPortal>,
    identities: IdentityStore,
    profiles: ProfileCache,
    account_id: AccountId,
}

impl std::fmt::Debug for Provisioner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provisioner").field("account", &self.account_id).finish_non_exhaustive()
    }
}

/// Redacts a UDID for plans and logs: the first and last four characters.
pub fn redact_udid(udid: &str) -> String {
    let n = udid.chars().count();
    if n <= 8 {
        return "****".into();
    }
    let head: String = udid.chars().take(4).collect();
    let tail: String = udid.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

fn valid_bundle_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

fn profile_type_for(d: Distribution) -> &'static str {
    match d {
        Distribution::Development => "IOS_APP_DEVELOPMENT",
        Distribution::AdHoc => "IOS_APP_ADHOC",
        Distribution::AppStore => "IOS_APP_STORE",
    }
}

fn min_validity(d: Distribution) -> Duration {
    match d {
        Distribution::Development => Duration::from_secs(24 * 3600),
        _ => Duration::from_secs(7 * 24 * 3600),
    }
}

fn russet_profile_name(d: Distribution, bundle: &str) -> String {
    let kind = match d {
        Distribution::Development => "Development",
        Distribution::AdHoc => "Ad Hoc",
        Distribution::AppStore => "App Store",
    };
    format!("Russet {kind} {bundle}")
}

impl Provisioner {
    pub fn new(
        portal: Arc<dyn DevPortal>,
        identities: IdentityStore,
        profiles: ProfileCache,
        account_id: AccountId,
    ) -> Self {
        Self { portal, identities, profiles, account_id }
    }

    pub fn team_id(&self) -> &str {
        self.portal.team_id()
    }

    /// GET-only.
    pub async fn plan(&self, r: &ProvisionRequest) -> Result<ProvisionPlan> {
        if !self.portal.can_provision() {
            return Err(Error::new(
                codes::account::INDIVIDUAL_KEY_NO_PROVISIONING,
                "individual API keys cannot use the provisioning endpoints",
            )
            .with_hint("Add a team key (App Manager or Admin) with `russet account add-key --issuer-id …`."));
        }
        if r.bundles.is_empty() {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "nothing to provision: no bundle")
                .with_hint("Set [app] bundle-id in russet.toml (or russet.local.toml)."));
        }
        let team = self.portal.team_id().to_owned();
        let d = r.distribution;
        let mut mutations = Vec::new();
        let mut warnings = Vec::new();

        // 1. The identity: a local one whose certificate the portal still lists and that has not expired.
        let wanted = if d == Distribution::Development { CertKind::Development } else { CertKind::Distribution };
        let portal_types: &[&str] = if wanted == CertKind::Development {
            &["DEVELOPMENT", "IOS_DEVELOPMENT"]
        } else {
            &["DISTRIBUTION", "IOS_DISTRIBUTION"]
        };
        let portal_serials: BTreeSet<String> = self
            .portal
            .list_certificates(None)
            .await?
            .into_iter()
            .filter(|c| portal_types.contains(&c.certificate_type.as_str()))
            .filter_map(|c| c.serial.map(|s| normalize_serial(&s)))
            .collect();
        let local: Vec<SigningIdentity> =
            self.identities.list(Some(&team))?.into_iter().filter(|i| i.kind == wanted && !i.is_expired()).collect();
        let identity = match &r.identity {
            Some(id) => {
                let i = local.iter().find(|i| &i.id == id).cloned().ok_or_else(|| {
                    Error::new(
                        codes::sign::NO_IDENTITY,
                        format!("{id} is not a usable {} identity of this team", wanted.as_str()),
                    )
                    .with_hint("List identities with `russet account list`.")
                })?;
                if !portal_serials.contains(&i.serial) {
                    warnings.push(PlanWarning {
                        message: format!("the portal no longer lists the certificate of {id} (revoked?)"),
                    });
                }
                Some(i)
            }
            None => local.iter().find(|i| portal_serials.contains(&i.serial)).cloned(),
        };
        if identity.is_none() {
            if !local.is_empty() {
                warnings.push(PlanWarning {
                    message:
                        "the local identities' certificates are not listed by the portal (revoked or of another team)"
                            .into(),
                });
            }
            mutations.push(Mutation::CreateCertificate { kind: wanted });
        }

        // 2. Devices (development and ad hoc).
        if d != Distribution::AppStore {
            if r.devices.is_empty() {
                warnings.push(PlanWarning {
                    message: "no device was given: the profiles' device lists were not checked".into(),
                });
            } else {
                let registered = self.portal.list_devices().await?;
                for udid in &r.devices {
                    if !registered.iter().any(|x| x.udid.eq_ignore_ascii_case(udid) && x.enabled()) {
                        mutations.push(Mutation::RegisterDevice { udid_redacted: redact_udid(udid) });
                        warnings.push(PlanWarning {
                            message: "new devices can take up to 72 hours to become usable on some memberships".into(),
                        });
                    }
                }
            }
        }

        // 3–5. Bundle IDs, capabilities, profiles.
        let mut profiles = Vec::new();
        for spec in &r.bundles {
            if !valid_bundle_id(&spec.bundle_id) {
                return Err(Error::new(
                    codes::build::BUNDLE_ID_INVALID,
                    format!("invalid bundle ID `{}`", spec.bundle_id),
                )
                .with_hint("Bundle IDs use letters, digits, `.` and `-`, with no empty component."));
            }
            let profile_name = russet_profile_name(d, &spec.bundle_id);
            let record = self.portal.find_bundle_id(&spec.bundle_id).await?;
            let Some(record) = record else {
                mutations.push(Mutation::RegisterBundleId {
                    identifier: spec.bundle_id.clone(),
                    name: format!("Russet {}", spec.bundle_id.replace(['.', '-', '_'], " ")),
                });
                for cap in requested_capabilities(spec) {
                    mutations.push(Mutation::EnableCapability { bundle: spec.bundle_id.clone(), capability: cap });
                }
                mutations.push(Mutation::CreateProfile {
                    name: profile_name,
                    kind: profile_type_for(d).into(),
                    bundle: spec.bundle_id.clone(),
                    devices: r.devices.len(),
                });
                continue;
            };
            let mut missing_caps = false;
            for cap in requested_capabilities(spec) {
                if !record.capabilities.contains(&cap) {
                    missing_caps = true;
                    mutations.push(Mutation::EnableCapability { bundle: spec.bundle_id.clone(), capability: cap });
                }
            }
            for key in spec.requested.keys() {
                if capability_for_entitlement(key).is_some_and(|c| c.needs_portal_step) {
                    warnings.push(PlanWarning {
                        message: format!(
                            "{key} needs a step in the developer portal (app groups and iCloud containers have no API): https://developer.apple.com/account/resources/identifiers/list"
                        ),
                    });
                }
            }
            let chosen = match (&identity, missing_caps) {
                (Some(i), false) => self.choose_profile(&record, spec, i, r, &team, &mut warnings).await?,
                _ => None,
            };
            match chosen {
                Some(p) => profiles.push(p),
                None => {
                    for stale in self.stale_russet_profiles(&record, &profile_name).await? {
                        mutations.push(stale);
                    }
                    mutations.push(Mutation::CreateProfile {
                        name: profile_name,
                        kind: profile_type_for(d).into(),
                        bundle: spec.bundle_id.clone(),
                        devices: r.devices.len(),
                    });
                }
            }
        }

        let identity_id = identity.map(|i| i.id);
        let digest = plan_digest(&self.account_id, &team, d, identity_id.as_ref(), &mutations, &profiles);
        Ok(ProvisionPlan {
            plan_id: PlanId::generate(),
            digest,
            team_id: team,
            mutations,
            warnings,
            account_id: self.account_id.clone(),
            distribution: d,
            identity: identity_id,
            profiles,
            request: r.clone(),
        })
    }

    /// An existing, usable profile of `record` (SIGN §6.4 step 5): Russet-named first, then any other (the
    /// Wave-1 slice reuses xtool's `XTL-…` profile), each checked for validity, certificate, devices, App ID and
    /// entitlements.
    async fn choose_profile(
        &self,
        record: &BundleIdRecord,
        spec: &BundleSpec,
        identity: &SigningIdentity,
        r: &ProvisionRequest,
        team: &str,
        warnings: &mut Vec<PlanWarning>,
    ) -> Result<Option<PlannedProfile>> {
        let d = r.distribution;
        let ty = profile_type_for(d);
        let russet_name = russet_profile_name(d, &spec.bundle_id);
        let mut candidates: Vec<_> = self
            .portal
            .profiles_for(record)
            .await?
            .into_iter()
            .filter(|p| p.profile_type == ty && p.active())
            .collect();
        candidates.sort_by_key(|p| {
            (p.name.as_deref() != Some(russet_name.as_str()), std::cmp::Reverse(p.expiration.clone()))
        });
        for c in candidates {
            let content = match c.content.clone() {
                Some(b) => b,
                None => match self.portal.profile(&c.id).await?.content {
                    Some(b) => b,
                    None => continue,
                },
            };
            let Ok(p) = ProvisioningProfile::parse(&content) else {
                warnings.push(PlanWarning { message: format!("profile {} could not be parsed and was skipped", c.id) });
                continue;
            };
            let resolves = EntitlementsResolver {
                profile: &p,
                bundle_id: &spec.bundle_id,
                distribution: d,
                policy: MergePolicy::Least,
            }
            .resolve(&spec.requested)
            .is_ok();
            let ok = p.kind == kind_for(d)
                && !p.expires_within(min_validity(d))
                && p.covers(&spec.bundle_id, team)
                && p.has_certificate(&identity.cert_der)
                && r.devices.iter().all(|u| p.has_device(u))
                && resolves;
            if ok {
                return Ok(Some(PlannedProfile {
                    rel_path: spec.rel_path.clone(),
                    bundle_id: spec.bundle_id.clone(),
                    profile_id: c.id,
                    uuid: p.uuid.clone(),
                    name: p.name.clone(),
                    expires: crate::fsutil::rfc3339(p.expiration),
                    content,
                }));
            }
        }
        Ok(None)
    }

    /// Russet-owned profiles of `record` that a new one replaces (never anybody else's).
    async fn stale_russet_profiles(&self, record: &BundleIdRecord, name: &str) -> Result<Vec<Mutation>> {
        Ok(self
            .portal
            .profiles_for(record)
            .await?
            .into_iter()
            .filter(|p| p.name.as_deref() == Some(name))
            .map(|p| Mutation::DeleteProfile { id: p.id, name: name.to_owned() })
            .collect())
    }

    /// Applies a plan (SIGN §6.4). A zero-mutation plan needs no approval; a plan with account changes needs an
    /// `approval` (the handler validates it against the plan's digest through the `ApprovalService`). Either way
    /// the plan is re-planned first (a staleness check against its digest): a drift answers `provision.planStale`.
    /// Then the mutations run in dependency order and every chosen profile — reused or just created — is
    /// downloaded and cached for `russet sign`.
    ///
    /// A plan with `CreateCertificate` creates the signing identity before anything else, so the new profiles can
    /// bind its certificate.
    pub async fn apply(&self, plan: &ProvisionPlan, approval: &ApprovalRef) -> Result<ProvisionOutcome> {
        if !plan.mutations.is_empty() && approval.approval_id.is_none() {
            let summary: Vec<String> = plan.mutations.iter().map(Mutation::summary).collect();
            return Err(Error::new(
                codes::perm::APPROVAL_REQUIRED,
                format!("the plan changes the developer account ({} change(s)): {}", summary.len(), summary.join("; ")),
            )
            .with_hint("Grant the approval (`russet approve …`, or Approve in Russet IDE), then apply again.")
            .with_details(serde_json::json!({"planId": plan.plan_id.0, "mutations": summary})));
        }
        let fresh = self.plan(&plan.request).await?;
        if fresh.digest != plan.digest {
            return Err(Error::new(
                codes::provision::PLAN_STALE,
                "the developer account changed since the plan was made",
            )
            .with_hint("Plan again: `russet provision --dry-run`, then apply the new plan.")
            .with_details(serde_json::json!({"planId": plan.plan_id.0, "reason": "drifted"})));
        }
        self.execute(&fresh, approval).await
    }

    /// Runs a (re-planned, digest-matched) plan's mutations and caches its profiles.
    async fn execute(&self, plan: &ProvisionPlan, approval: &ApprovalRef) -> Result<ProvisionOutcome> {
        let creating = plan.mutations.iter().any(|m| matches!(m, Mutation::CreateProfile { .. }));

        // A plan without a usable local identity creates one first (SIGN §6.3): a new key, sealed before Apple
        // sees its CSR, and the certificate Apple issues for it. New profiles bind to that certificate.
        let mut identity = plan.identity.clone();
        let mut created_cert = None;
        for m in &plan.mutations {
            if let Mutation::CreateCertificate { kind } = m {
                let (pending, csr) = self.identities.new_csr_async(self.portal.team_id(), *kind).await?;
                let cert = self.portal.create_certificate(kind.certificate_type(), &csr, approval).await?;
                let der = cert.content_der.as_deref().ok_or_else(|| {
                    Error::new(
                        codes::account::APPLE_SERVICE_UNAVAILABLE,
                        "Apple created the certificate but did not return it; its key stays in the secret store",
                    )
                    .with_hint("Plan again: the certificate is listed by the portal and can be downloaded.")
                })?;
                let stored = self.identities.commit_async(pending, der, Some(cert.id.clone())).await?;
                identity = Some(stored.id);
                created_cert = Some(cert.id);
            }
        }

        // The portal certificate id of the plan's identity, needed to bind new profiles.
        let cert_ids = match (creating, created_cert) {
            (false, _) => Vec::new(),
            (true, Some(id)) => vec![id],
            (true, None) => {
                let id = identity.as_ref().ok_or_else(|| {
                    Error::new(codes::sign::NO_IDENTITY, "the plan has no signing identity to bind the new profiles to")
                        .with_hint("Add an identity (`russet account import --from-xtool` or a .p12) and plan again.")
                })?;
                vec![self.portal_certificate_id(id, plan.distribution).await?]
            }
        };

        // Development and ad hoc profiles bind every enabled mobile device (SIGN §6.4 step 5); register the
        // selected UDIDs that are missing first.
        let device_ids = if creating && plan.distribution != Distribution::AppStore {
            let registered = self.portal.list_devices().await?;
            for udid in &plan.request.devices {
                if !registered.iter().any(|x| x.udid.eq_ignore_ascii_case(udid) && x.enabled()) {
                    self.portal.register_device(udid, &format!("Russet {}", redact_udid(udid)), approval).await?;
                }
            }
            self.portal
                .list_devices()
                .await?
                .into_iter()
                .filter(|d| d.enabled() && is_mobile_class(d))
                .map(|d| d.id)
                .collect()
        } else {
            Vec::new()
        };

        // Delete stale Russet profiles before recreating (only Russet-named ones are ever planned).
        for m in &plan.mutations {
            if let Mutation::DeleteProfile { id, .. } = m {
                self.portal.delete_profile(id, approval).await?;
            }
        }

        // Register bundle IDs, then enable their missing capabilities.
        let mut records: std::collections::BTreeMap<String, BundleIdRecord> = std::collections::BTreeMap::new();
        for m in &plan.mutations {
            if let Mutation::RegisterBundleId { identifier, name } = m {
                let rec = self.portal.register_bundle_id(identifier, name, approval).await?;
                records.insert(identifier.clone(), rec);
            }
        }
        for m in &plan.mutations {
            if let Mutation::EnableCapability { bundle, capability } = m {
                let rec = self.bundle_record(&records, bundle).await?;
                self.portal.enable_capability(&rec, capability, approval).await?;
                records.insert(bundle.clone(), rec);
            }
        }

        // Cache the reused profiles, then create and cache the new ones.
        let mut bundles = Vec::new();
        for p in &plan.profiles {
            let (profile, path) = self.profiles.put(&p.content, Some(&p.profile_id))?;
            bundles.push(BundleProvision {
                rel_path: p.rel_path.clone(),
                bundle_id: p.bundle_id.clone(),
                profile_uuid: profile.uuid,
                profile_path: path,
            });
        }
        for m in &plan.mutations {
            if let Mutation::CreateProfile { name, kind, bundle, .. } = m {
                let rec = self.bundle_record(&records, bundle).await?;
                let created = self.portal.create_profile(name, kind, &rec, &cert_ids, &device_ids, approval).await?;
                let content = match created.content {
                    Some(c) => c,
                    None => self.portal.profile(&created.id).await?.content.ok_or_else(|| {
                        Error::new(
                            codes::account::APPLE_SERVICE_UNAVAILABLE,
                            "the created profile has no content to cache",
                        )
                        .with_hint("Retry later; report it with `russet bugreport` if it persists.")
                    })?,
                };
                let (profile, path) = self.profiles.put(&content, Some(&created.id))?;
                let rel_path = plan
                    .request
                    .bundles
                    .iter()
                    .find(|s| &s.bundle_id == bundle)
                    .map_or_else(String::new, |s| s.rel_path.clone());
                bundles.push(BundleProvision {
                    rel_path,
                    bundle_id: bundle.clone(),
                    profile_uuid: profile.uuid,
                    profile_path: path,
                });
            }
        }
        bundles.sort_by_key(|b| {
            plan.request.bundles.iter().position(|s| s.bundle_id == b.bundle_id).unwrap_or(usize::MAX)
        });
        Ok(ProvisionOutcome { identity, bundles })
    }

    /// The bundle-ID record for `bundle`: one just registered, else looked up (it must already exist).
    async fn bundle_record(
        &self,
        registered: &std::collections::BTreeMap<String, BundleIdRecord>,
        bundle: &str,
    ) -> Result<BundleIdRecord> {
        if let Some(r) = registered.get(bundle) {
            return Ok(r.clone());
        }
        self.portal.find_bundle_id(bundle).await?.ok_or_else(|| {
            Error::new(codes::core::NOT_FOUND, format!("the bundle ID {bundle} is not registered"))
                .with_hint("Plan again (`russet provision --dry-run`); the account changed since the plan was made.")
        })
    }

    /// The portal certificate id of a local identity (matched by serial), for binding new profiles.
    async fn portal_certificate_id(&self, id: &IdentityId, d: Distribution) -> Result<String> {
        let identity = self.identities.get(id)?;
        let wanted = normalize_serial(&identity.serial);
        self.portal
            .list_certificates(None)
            .await?
            .into_iter()
            .find(|c| c.serial.as_deref().map(normalize_serial) == Some(wanted.clone()))
            .map(|c| c.id)
            .ok_or_else(|| {
                Error::new(
                    codes::sign::NO_IDENTITY,
                    format!("the portal no longer lists the {} certificate the plan chose", d.as_str()),
                )
                .with_hint("Plan again (`russet provision --dry-run`); the certificate may have been revoked.")
            })
    }
}

/// Whether a device is a class a development/ad-hoc profile lists (iPhone, iPad or iPod).
fn is_mobile_class(d: &crate::portal::DeviceRecord) -> bool {
    matches!(d.device_class.as_deref(), Some("IPHONE" | "IPAD" | "IPOD"))
}

/// The capability types a bundle's requested entitlements need.
fn requested_capabilities(spec: &BundleSpec) -> Vec<String> {
    let mut caps: Vec<String> =
        spec.requested.keys().filter_map(|k| capability_for_entitlement(k)).map(|c| c.capability.to_owned()).collect();
    caps.sort();
    caps.dedup();
    caps
}

fn normalize_serial(s: &str) -> String {
    let up = s.trim().to_uppercase();
    let t = up.trim_start_matches('0');
    if t.is_empty() { "0".into() } else { t.into() }
}

/// SHA-256 over a canonical JSON of what an approval approves: account, team, distribution, identity, mutations
/// and the reused profiles (warnings and the plan id excluded).
fn plan_digest(
    account: &AccountId,
    team: &str,
    d: Distribution,
    identity: Option<&IdentityId>,
    mutations: &[Mutation],
    profiles: &[PlannedProfile],
) -> [u8; 32] {
    let value = serde_json::json!({
        "account": account.0,
        "team": team,
        "distribution": d.as_str(),
        "identity": identity.map(|i| i.0.clone()),
        "mutations": mutations.iter().map(|m| format!("{m:?}")).collect::<Vec<_>>(),
        "profiles": profiles.iter().map(|p| serde_json::json!({
            "rel": p.rel_path, "bundle": p.bundle_id, "id": p.profile_id, "uuid": p.uuid
        })).collect::<Vec<_>>(),
    });
    let bytes = serde_json::to_vec(&value).unwrap_or_default();
    sha2::Sha256::digest(&bytes).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_specs_read_entitlements() {
        let dir = std::env::temp_dir().join(format!("russet-bundlespec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let p = dir.join("App.entitlements");
        let mut d = plist::Dictionary::new();
        d.insert("aps-environment".into(), plist::Value::String("development".into()));
        plist::Value::Dictionary(d).to_file_xml(&p).unwrap_or_default();
        let spec = BundleSpec::with_entitlements_file("", "com.example.app", Some(&p));
        assert!(spec.is_ok_and(|s| s.requested.contains_key("aps-environment")));
        assert!(BundleSpec::with_entitlements_file("", "x", None).is_ok_and(|s| s.requested.is_empty()));
        assert!(BundleSpec::with_entitlements_file("", "x", Some(&dir.join("missing"))).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn helpers() {
        assert_eq!(redact_udid("00008101-000A1B2C3D4E5F60"), "0000…5F60");
        assert_eq!(redact_udid("short"), "****");
        assert!(valid_bundle_id("XTL-69A6DE7D.com.example.hello"));
        assert!(!valid_bundle_id("com..x") && !valid_bundle_id(".x") && !valid_bundle_id("a b"));
        assert_eq!(normalize_serial("00ab"), "AB");
        assert!(PlanId::generate().0.starts_with("pln_"));
    }
}
