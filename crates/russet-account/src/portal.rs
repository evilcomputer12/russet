// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The developer portal of one team (SIGN §6.2): one surface for paid (App Store Connect API, here) and free
//! (DS2, W22) teams. Wave 1 reads only; the mutating operations exist so W12 can implement them, and refuse.

use async_trait::async_trait;
use russet_asc::model::{BundleIdAttributes, CertificateAttributes, DeviceAttributes, ProfileAttributes, Resource};
use russet_asc::{ApiFlavor, AscClient, CertificateFilter, DeviceFilter, ProfileFilter};
use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::provision::ApprovalRef;
use crate::store::base64_std;

/// A certificate as the portal lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertInfo {
    pub id: String,
    /// `DEVELOPMENT`, `DISTRIBUTION`, … (open: unknown types are kept).
    pub certificate_type: String,
    pub serial: Option<String>,
    pub display_name: Option<String>,
    pub expiration: Option<String>,
    pub content_der: Option<Vec<u8>>,
}

/// A registered device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRecord {
    pub id: String,
    pub udid: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub device_class: Option<String>,
    pub status: Option<String>,
    pub model: Option<String>,
}

impl DeviceRecord {
    pub fn enabled(&self) -> bool {
        self.status.as_deref() != Some("DISABLED")
    }
}

/// A registered bundle ID with its enabled capability types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleIdRecord {
    pub id: String,
    pub identifier: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub seed_id: Option<String>,
    pub capabilities: Vec<String>,
}

/// A profile as the portal lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRecord {
    pub id: String,
    pub name: Option<String>,
    /// `IOS_APP_DEVELOPMENT`, …
    pub profile_type: String,
    /// `ACTIVE` or `INVALID`.
    pub state: Option<String>,
    pub uuid: Option<String>,
    pub expiration: Option<String>,
    /// The decoded `.mobileprovision`, when the listing included it.
    pub content: Option<Vec<u8>>,
}

impl ProfileRecord {
    pub fn active(&self) -> bool {
        self.state.as_deref() != Some("INVALID")
    }
}

/// Which portal a team is reached through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalFlavor {
    PublicApi,
    XcodeDs2,
}

/// The developer portal of one team (ASC team key, or the free Apple ID's DS2 session).
#[async_trait]
pub trait DevPortal: Send + Sync {
    fn team_id(&self) -> &str;
    fn flavor(&self) -> PortalFlavor;
    /// Individual keys cannot use the provisioning endpoints.
    fn can_provision(&self) -> bool;
    async fn list_certificates(&self, certificate_type: Option<&str>) -> Result<Vec<CertInfo>>;
    async fn list_devices(&self) -> Result<Vec<DeviceRecord>>;
    /// The bundle ID whose identifier is exactly `identifier` (the API's filter is a prefix match [V5]).
    async fn find_bundle_id(&self, identifier: &str) -> Result<Option<BundleIdRecord>>;
    async fn list_bundle_ids(&self) -> Result<Vec<BundleIdRecord>>;
    async fn list_profiles(&self, profile_type: Option<&str>) -> Result<Vec<ProfileRecord>>;
    /// The profiles attached to a bundle ID.
    async fn profiles_for(&self, bundle: &BundleIdRecord) -> Result<Vec<ProfileRecord>>;
    /// One profile with its content.
    async fn profile(&self, id: &str) -> Result<ProfileRecord>;

    // Mutations: W12 implements them behind an approved plan (ARCHITECTURE §8.4). The default refuses, so a
    // portal that cannot mutate (the free-team DS2 portal of W22, until it implements them) stays safe.
    async fn register_device(&self, _udid: &str, _name: &str, _approval: &ApprovalRef) -> Result<DeviceRecord> {
        Err(mutations_later())
    }
    async fn register_bundle_id(
        &self,
        _identifier: &str,
        _name: &str,
        _approval: &ApprovalRef,
    ) -> Result<BundleIdRecord> {
        Err(mutations_later())
    }
    async fn enable_capability(
        &self,
        _bundle: &BundleIdRecord,
        _capability: &str,
        _approval: &ApprovalRef,
    ) -> Result<()> {
        Err(mutations_later())
    }
    /// Creates a profile bound to `bundle`, the given certificates and (development/ad hoc) devices.
    async fn create_profile(
        &self,
        _name: &str,
        _kind: &str,
        _bundle: &BundleIdRecord,
        _certificate_ids: &[String],
        _device_ids: &[String],
        _approval: &ApprovalRef,
    ) -> Result<ProfileRecord> {
        Err(mutations_later())
    }
    /// Deletes a profile by its portal id (only Russet-named stale ones are ever planned).
    async fn delete_profile(&self, _id: &str, _approval: &ApprovalRef) -> Result<()> {
        Err(mutations_later())
    }
    /// Revokes a certificate by its portal id (`portal.certificates.revoke`, typed confirmation).
    async fn revoke_certificate(&self, _id: &str, _approval: &ApprovalRef) -> Result<()> {
        Err(mutations_later())
    }
}

pub(crate) fn mutations_later() -> Error {
    Error::new(codes::core::NOT_IMPLEMENTED, "Russet does not change the developer account yet")
        .with_hint("Wave 1 only reads App Store Connect; approved account changes arrive with W12.")
}

/// The public App Store Connect API with a team key: a per-request GET-scoped `client` for reads, and a
/// bearer-token `mut_client` for the writes an approved plan performs (W12).
pub struct AscPortal {
    client: AscClient,
    mut_client: AscClient,
    team_id: String,
    individual: bool,
}

impl std::fmt::Debug for AscPortal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AscPortal").finish_non_exhaustive()
    }
}

impl AscPortal {
    pub fn new(client: AscClient, mut_client: AscClient, team_id: String, individual: bool) -> Self {
        Self { client, mut_client, team_id, individual }
    }

    pub fn client(&self) -> &AscClient {
        &self.client
    }
}

/// A JSON:API `relationships` entry pointing at one resource (`{"data":{"type":t,"id":id}}`).
fn rel_one(t: &str, id: &str) -> serde_json::Value {
    serde_json::json!({"data": {"type": t, "id": id}})
}

/// A JSON:API `relationships` entry pointing at many resources.
fn rel_many(t: &str, ids: &[String]) -> serde_json::Value {
    serde_json::json!({"data": ids.iter().map(|id| serde_json::json!({"type": t, "id": id})).collect::<Vec<_>>()})
}

fn cert_info(r: Resource<CertificateAttributes>) -> CertInfo {
    let a = r.attributes;
    CertInfo {
        id: r.id,
        certificate_type: a.as_ref().map(|a| a.certificate_type.as_wire()).unwrap_or_default(),
        serial: a.as_ref().and_then(|a| a.serial_number.clone()),
        display_name: a.as_ref().and_then(|a| a.display_name.clone()),
        expiration: a.as_ref().and_then(|a| a.expiration_date.clone()),
        content_der: a.as_ref().and_then(|a| a.certificate_content.as_deref()).and_then(base64_std),
    }
}

fn device_record(r: Resource<DeviceAttributes>) -> Option<DeviceRecord> {
    let a = r.attributes?;
    Some(DeviceRecord {
        id: r.id,
        udid: a.udid,
        name: a.name,
        platform: a.platform.map(|p| p.as_wire()),
        device_class: a.device_class.map(|d| d.as_wire()),
        status: a.status.map(|s| s.as_wire()),
        model: a.model,
    })
}

fn bundle_record(r: &Resource<BundleIdAttributes>, capabilities: Vec<String>) -> Option<BundleIdRecord> {
    let a = r.attributes.as_ref()?;
    Some(BundleIdRecord {
        id: r.id.clone(),
        identifier: a.identifier.clone(),
        name: a.name.clone(),
        platform: a.platform.as_ref().map(|p| p.as_wire()),
        seed_id: a.seed_id.clone(),
        capabilities,
    })
}

fn profile_record(r: Resource<ProfileAttributes>) -> Option<ProfileRecord> {
    let a = r.attributes?;
    Some(ProfileRecord {
        id: r.id,
        name: a.name,
        profile_type: a.profile_type.as_wire(),
        state: a.profile_state.map(|s| s.as_wire()),
        uuid: a.uuid,
        expiration: a.expiration_date,
        content: a.profile_content.as_deref().and_then(base64_std),
    })
}

#[async_trait]
impl DevPortal for AscPortal {
    fn team_id(&self) -> &str {
        &self.team_id
    }

    fn flavor(&self) -> PortalFlavor {
        match self.client.flavor() {
            ApiFlavor::Public => PortalFlavor::PublicApi,
            ApiFlavor::XcodeDs2 { .. } => PortalFlavor::XcodeDs2,
        }
    }

    fn can_provision(&self) -> bool {
        !self.individual
    }

    async fn list_certificates(&self, certificate_type: Option<&str>) -> Result<Vec<CertInfo>> {
        let f = CertificateFilter { certificate_type: certificate_type.map(str::to_owned), ..Default::default() };
        Ok(self.client.certificates().list(&f).await?.into_iter().map(cert_info).collect())
    }

    async fn list_devices(&self) -> Result<Vec<DeviceRecord>> {
        Ok(self.client.devices().list(&DeviceFilter::default()).await?.into_iter().filter_map(device_record).collect())
    }

    async fn find_bundle_id(&self, identifier: &str) -> Result<Option<BundleIdRecord>> {
        let found = self.client.bundle_ids().find_exact(identifier).await?;
        Ok(found.and_then(|r| {
            let caps = r.capability_types();
            bundle_record(&r.resource, caps)
        }))
    }

    async fn list_bundle_ids(&self) -> Result<Vec<BundleIdRecord>> {
        let all = self.client.bundle_ids().list(None).await?;
        Ok(all.iter().filter_map(|r| bundle_record(&r.resource, r.capability_types())).collect())
    }

    async fn list_profiles(&self, profile_type: Option<&str>) -> Result<Vec<ProfileRecord>> {
        let f = ProfileFilter { profile_type: profile_type.map(str::to_owned), ..Default::default() };
        Ok(self.client.profiles().list(&f).await?.into_iter().filter_map(profile_record).collect())
    }

    async fn profiles_for(&self, bundle: &BundleIdRecord) -> Result<Vec<ProfileRecord>> {
        Ok(self.client.bundle_ids().profiles(&bundle.id).await?.into_iter().filter_map(profile_record).collect())
    }

    async fn profile(&self, id: &str) -> Result<ProfileRecord> {
        profile_record(self.client.profiles().get(id).await?).ok_or_else(|| {
            Error::new(
                codes::account::APPLE_SERVICE_UNAVAILABLE,
                "App Store Connect returned a profile without attributes",
            )
            .with_hint("Retry later; report it with `russet bugreport` if it persists.")
        })
    }

    async fn register_device(&self, udid: &str, name: &str, _approval: &ApprovalRef) -> Result<DeviceRecord> {
        let body = serde_json::json!({
            "data": {"type": "devices", "attributes": {"name": name, "platform": "IOS", "udid": udid}}
        });
        let doc: russet_asc::model::Document<Resource<DeviceAttributes>> =
            self.mut_client.post_json("/v1/devices", &body).await?;
        device_record(doc.data).ok_or_else(|| missing_attributes("device"))
    }

    async fn register_bundle_id(
        &self,
        identifier: &str,
        name: &str,
        _approval: &ApprovalRef,
    ) -> Result<BundleIdRecord> {
        let body = serde_json::json!({
            "data": {"type": "bundleIds", "attributes": {"identifier": identifier, "name": name, "platform": "IOS"}}
        });
        let doc: russet_asc::model::Document<Resource<BundleIdAttributes>> =
            self.mut_client.post_json("/v1/bundleIds", &body).await?;
        bundle_record(&doc.data, Vec::new()).ok_or_else(|| missing_attributes("bundle ID"))
    }

    async fn enable_capability(
        &self,
        bundle: &BundleIdRecord,
        capability: &str,
        _approval: &ApprovalRef,
    ) -> Result<()> {
        let body = serde_json::json!({
            "data": {
                "type": "bundleIdCapabilities",
                "attributes": {"capabilityType": capability},
                "relationships": {"bundleId": rel_one("bundleIds", &bundle.id)}
            }
        });
        self.mut_client.post_json::<serde_json::Value>("/v1/bundleIdCapabilities", &body).await?;
        Ok(())
    }

    async fn create_profile(
        &self,
        name: &str,
        kind: &str,
        bundle: &BundleIdRecord,
        certificate_ids: &[String],
        device_ids: &[String],
        _approval: &ApprovalRef,
    ) -> Result<ProfileRecord> {
        let mut relationships = serde_json::Map::new();
        relationships.insert("bundleId".into(), rel_one("bundleIds", &bundle.id));
        relationships.insert("certificates".into(), rel_many("certificates", certificate_ids));
        if !device_ids.is_empty() {
            relationships.insert("devices".into(), rel_many("devices", device_ids));
        }
        let body = serde_json::json!({
            "data": {
                "type": "profiles",
                "attributes": {"name": name, "profileType": kind},
                "relationships": relationships
            }
        });
        let doc: russet_asc::model::Document<Resource<ProfileAttributes>> =
            self.mut_client.post_json("/v1/profiles", &body).await?;
        profile_record(doc.data).ok_or_else(|| missing_attributes("profile"))
    }

    async fn delete_profile(&self, id: &str, _approval: &ApprovalRef) -> Result<()> {
        self.mut_client.delete(&format!("/v1/profiles/{id}")).await
    }

    async fn revoke_certificate(&self, id: &str, _approval: &ApprovalRef) -> Result<()> {
        self.mut_client.delete(&format!("/v1/certificates/{id}")).await
    }
}

fn missing_attributes(what: &str) -> Error {
    Error::new(
        codes::account::APPLE_SERVICE_UNAVAILABLE,
        format!("App Store Connect returned a {what} without attributes"),
    )
    .with_hint("Retry later; report it with `russet bugreport` if it persists.")
}
