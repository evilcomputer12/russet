// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Typed, GET-only resource accessors (SIGN §5.4): certificates, devices, bundle IDs and their capabilities,
//! profiles, apps, builds, TestFlight groups and Xcode Cloud products. Exact-match helpers post-filter the
//! prefix matches App Store Connect returns for `filter[identifier]` [V5].

use russet_core::Result;
use russet_core::error::{Error, codes};
use serde::de::DeserializeOwned;

use crate::client::AscClient;
use crate::model::*;

/// The page size Russet asks for.
pub const PAGE_LIMIT: u32 = 200;

/// Appends `key=value` (value form-encoded) to a query.
fn push(q: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(v) = value {
        let enc: String = url::form_urlencoded::byte_serialize(v.as_bytes()).collect();
        q.push(format!("{key}={enc}"));
    }
}

fn path(base: &str, q: Vec<String>) -> String {
    if q.is_empty() { base.to_owned() } else { format!("{base}?{}", q.join("&")) }
}

fn limit(q: &mut Vec<String>, n: u32) {
    q.push(format!("limit={}", n.clamp(1, PAGE_LIMIT)));
}

/// `certificates` filters.
#[derive(Debug, Clone, Default)]
pub struct CertificateFilter {
    /// `filter[certificateType]`, for example `DEVELOPMENT`.
    pub certificate_type: Option<String>,
    /// `filter[serialNumber]` (uppercase hex without leading zeros).
    pub serial_number: Option<String>,
}

/// `devices` filters.
#[derive(Debug, Clone, Default)]
pub struct DeviceFilter {
    pub udid: Option<String>,
    /// `IOS`, `MAC_OS`, …
    pub platform: Option<String>,
    /// `ENABLED` or `DISABLED`.
    pub status: Option<String>,
}

/// `profiles` filters.
#[derive(Debug, Clone, Default)]
pub struct ProfileFilter {
    pub name: Option<String>,
    /// `IOS_APP_DEVELOPMENT`, …
    pub profile_type: Option<String>,
    /// `ACTIVE` or `INVALID`.
    pub profile_state: Option<String>,
}

/// `/v1/certificates`.
#[derive(Debug, Clone, Copy)]
pub struct CertificatesApi<'a>(&'a AscClient);
/// `/v1/devices`.
#[derive(Debug, Clone, Copy)]
pub struct DevicesApi<'a>(&'a AscClient);
/// `/v1/bundleIds` and their capabilities.
#[derive(Debug, Clone, Copy)]
pub struct BundleIdsApi<'a>(&'a AscClient);
/// `/v1/profiles`.
#[derive(Debug, Clone, Copy)]
pub struct ProfilesApi<'a>(&'a AscClient);
/// `/v1/apps`.
#[derive(Debug, Clone, Copy)]
pub struct AppsApi<'a>(&'a AscClient);
/// `/v1/builds`.
#[derive(Debug, Clone, Copy)]
pub struct BuildsApi<'a>(&'a AscClient);
/// TestFlight resources (`/v1/betaGroups`, …).
#[derive(Debug, Clone, Copy)]
pub struct TestFlightApi<'a>(&'a AscClient);
/// Xcode Cloud resources (`/v1/ciProducts`, …).
#[derive(Debug, Clone, Copy)]
pub struct CiApi<'a>(&'a AscClient);

impl AscClient {
    pub fn certificates(&self) -> CertificatesApi<'_> {
        CertificatesApi(self)
    }
    pub fn devices(&self) -> DevicesApi<'_> {
        DevicesApi(self)
    }
    pub fn bundle_ids(&self) -> BundleIdsApi<'_> {
        BundleIdsApi(self)
    }
    pub fn profiles(&self) -> ProfilesApi<'_> {
        ProfilesApi(self)
    }
    pub fn apps(&self) -> AppsApi<'_> {
        AppsApi(self)
    }
    pub fn builds(&self) -> BuildsApi<'_> {
        BuildsApi(self)
    }
    pub fn testflight(&self) -> TestFlightApi<'_> {
        TestFlightApi(self)
    }
    pub fn ci(&self) -> CiApi<'_> {
        CiApi(self)
    }

    async fn list<A: DeserializeOwned>(&self, p: &str) -> Result<Vec<Resource<A>>> {
        Ok(self.get_all::<A>(p).await?.0)
    }

    async fn one<A: DeserializeOwned>(&self, p: &str) -> Result<Resource<A>> {
        Ok(self.get_resource::<A>(p).await?.data)
    }
}

impl CertificatesApi<'_> {
    pub async fn list(&self, f: &CertificateFilter) -> Result<Vec<Resource<CertificateAttributes>>> {
        let mut q = Vec::new();
        push(&mut q, "filter[certificateType]", f.certificate_type.as_deref());
        push(&mut q, "filter[serialNumber]", f.serial_number.as_deref());
        limit(&mut q, PAGE_LIMIT);
        self.0.list(&path("/v1/certificates", q)).await
    }

    /// One page of at most `n` certificates (the `account.verify` probe).
    pub async fn probe(&self, n: u32) -> Result<Vec<Resource<CertificateAttributes>>> {
        let doc: Document<Vec<Resource<CertificateAttributes>>> =
            self.0.get_json(&format!("/v1/certificates?limit={}", n.clamp(1, PAGE_LIMIT))).await?;
        Ok(doc.data)
    }

    pub async fn get(&self, id: &str) -> Result<Resource<CertificateAttributes>> {
        self.0.one(&format!("/v1/certificates/{}", segment(id)?)).await
    }
}

impl DevicesApi<'_> {
    pub async fn list(&self, f: &DeviceFilter) -> Result<Vec<Resource<DeviceAttributes>>> {
        let mut q = Vec::new();
        push(&mut q, "filter[udid]", f.udid.as_deref());
        push(&mut q, "filter[platform]", f.platform.as_deref());
        push(&mut q, "filter[status]", f.status.as_deref());
        limit(&mut q, PAGE_LIMIT);
        self.0.list(&path("/v1/devices", q)).await
    }
}

impl BundleIdsApi<'_> {
    /// Bundle IDs whose identifier starts with `identifier_prefix` (App Store Connect's `filter[identifier]`
    /// is a prefix match [V5]), with their capabilities.
    pub async fn list(&self, identifier_prefix: Option<&str>) -> Result<Vec<BundleIdRecord>> {
        let mut q = Vec::new();
        push(&mut q, "filter[identifier]", identifier_prefix);
        q.push("include=bundleIdCapabilities".into());
        limit(&mut q, PAGE_LIMIT);
        let (resources, included) = self.0.get_all::<BundleIdAttributes>(&path("/v1/bundleIds", q)).await?;
        let caps: Vec<Resource<BundleIdCapabilityAttributes>> = included
            .into_iter()
            .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("bundleIdCapabilities"))
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        Ok(resources
            .into_iter()
            .map(|r| {
                let ids = r.related_ids("bundleIdCapabilities");
                let capabilities = caps.iter().filter(|c| ids.contains(&c.id)).cloned().collect();
                BundleIdRecord { resource: r, capabilities }
            })
            .collect())
    }

    /// The bundle ID whose identifier is exactly `identifier`; `account.ascAmbiguous` when several are.
    pub async fn find_exact(&self, identifier: &str) -> Result<Option<BundleIdRecord>> {
        let mut exact: Vec<BundleIdRecord> =
            self.list(Some(identifier)).await?.into_iter().filter(|r| r.identifier() == identifier).collect();
        match exact.len() {
            0 => Ok(None),
            1 => Ok(exact.pop()),
            n => {
                Err(Error::new(codes::account::ASC_AMBIGUOUS, format!("{n} bundle IDs are registered as {identifier}"))
                    .with_hint("Remove the duplicates in the developer portal (Identifiers), then retry."))
            }
        }
    }

    pub async fn get(&self, id: &str) -> Result<Resource<BundleIdAttributes>> {
        self.0.one(&format!("/v1/bundleIds/{}", segment(id)?)).await
    }

    /// `GET /v1/bundleIds/{id}/bundleIdCapabilities`.
    pub async fn capabilities(&self, id: &str) -> Result<Vec<Resource<BundleIdCapabilityAttributes>>> {
        self.0.list(&format!("/v1/bundleIds/{}/bundleIdCapabilities?limit={PAGE_LIMIT}", segment(id)?)).await
    }

    /// `GET /v1/bundleIds/{id}/profiles`.
    pub async fn profiles(&self, id: &str) -> Result<Vec<Resource<ProfileAttributes>>> {
        self.0.list(&format!("/v1/bundleIds/{}/profiles?limit={PAGE_LIMIT}", segment(id)?)).await
    }
}

impl ProfilesApi<'_> {
    pub async fn list(&self, f: &ProfileFilter) -> Result<Vec<Resource<ProfileAttributes>>> {
        let mut q = Vec::new();
        push(&mut q, "filter[name]", f.name.as_deref());
        push(&mut q, "filter[profileType]", f.profile_type.as_deref());
        push(&mut q, "filter[profileState]", f.profile_state.as_deref());
        limit(&mut q, PAGE_LIMIT);
        self.0.list(&path("/v1/profiles", q)).await
    }

    /// One profile, `profileContent` included.
    pub async fn get(&self, id: &str) -> Result<Resource<ProfileAttributes>> {
        self.0.one(&format!("/v1/profiles/{}", segment(id)?)).await
    }

    /// `GET /v1/profiles/{id}/bundleId`.
    pub async fn bundle_id(&self, id: &str) -> Result<Resource<BundleIdAttributes>> {
        self.0.one(&format!("/v1/profiles/{}/bundleId", segment(id)?)).await
    }

    /// `GET /v1/profiles/{id}/certificates`.
    pub async fn certificates(&self, id: &str) -> Result<Vec<Resource<CertificateAttributes>>> {
        self.0.list(&format!("/v1/profiles/{}/certificates?limit={PAGE_LIMIT}", segment(id)?)).await
    }

    /// `GET /v1/profiles/{id}/devices`.
    pub async fn devices(&self, id: &str) -> Result<Vec<Resource<DeviceAttributes>>> {
        self.0.list(&format!("/v1/profiles/{}/devices?limit={PAGE_LIMIT}", segment(id)?)).await
    }
}

impl AppsApi<'_> {
    pub async fn list(&self, bundle_id: Option<&str>) -> Result<Vec<Resource<AppAttributes>>> {
        let mut q = Vec::new();
        push(&mut q, "filter[bundleId]", bundle_id);
        limit(&mut q, PAGE_LIMIT);
        self.0.list(&path("/v1/apps", q)).await
    }
}

impl BuildsApi<'_> {
    /// The most recent `n` builds (one page).
    pub async fn recent(&self, n: u32) -> Result<Vec<Resource<BuildAttributes>>> {
        let doc: Document<Vec<Resource<BuildAttributes>>> =
            self.0.get_json(&format!("/v1/builds?limit={}", n.clamp(1, PAGE_LIMIT))).await?;
        Ok(doc.data)
    }
}

impl TestFlightApi<'_> {
    pub async fn beta_groups(&self) -> Result<Vec<Resource<BetaGroupAttributes>>> {
        self.0.list(&format!("/v1/betaGroups?limit={PAGE_LIMIT}")).await
    }
}

impl CiApi<'_> {
    pub async fn products(&self) -> Result<Vec<Resource<CiProductAttributes>>> {
        self.0.list(&format!("/v1/ciProducts?limit={PAGE_LIMIT}")).await
    }
}

/// A resource ID as a path segment: App Store Connect IDs are alphanumeric (with `-`); anything else is
/// refused rather than escaped.
fn segment(id: &str) -> Result<&str> {
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Ok(id)
    } else {
        Err(Error::new(codes::core::INVALID_ARGUMENT, format!("invalid App Store Connect id `{id}`"))
            .with_hint("Use a resource id as `russet account certs|devices|bundle-ids|profiles list` shows it."))
    }
}
