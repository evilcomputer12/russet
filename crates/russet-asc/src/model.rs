// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! JSON:API documents and the typed attributes of the resources Russet reads (SIGN §5.3–§5.4). Every enum is
//! open ([`Open`]): Apple returns values the published spec lacks [V5].

use std::collections::BTreeMap;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An open enum: a known value, or the string Apple sent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Open<E> {
    Known(E),
    Unknown(String),
}

impl<E: Serialize + DeserializeOwned + Clone> Open<E> {
    /// The wire string.
    pub fn as_wire(&self) -> String {
        match self {
            Self::Known(e) => {
                serde_json::to_value(e).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
            }
            Self::Unknown(s) => s.clone(),
        }
    }

    pub fn known(&self) -> Option<&E> {
        match self {
            Self::Known(e) => Some(e),
            Self::Unknown(_) => None,
        }
    }
}

impl<E: PartialEq> Open<E> {
    pub fn is(&self, e: &E) -> bool {
        matches!(self, Self::Known(k) if k == e)
    }
}

impl<'de, E: DeserializeOwned> Deserialize<'de> for Open<E> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match serde_json::from_value::<E>(serde_json::Value::String(s.clone())) {
            Ok(e) => Self::Known(e),
            Err(_) => Self::Unknown(s),
        })
    }
}

impl<E: Serialize> Serialize for Open<E> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Known(e) => e.serialize(s),
            Self::Unknown(u) => s.serialize_str(u),
        }
    }
}

impl<E: fmt::Debug> fmt::Display for Open<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(e) => write!(f, "{e:?}"),
            Self::Unknown(s) => f.write_str(s),
        }
    }
}

/// A top-level JSON:API document.
#[derive(Debug, Clone, Deserialize)]
pub struct Document<T> {
    pub data: T,
    #[serde(default)]
    pub included: Vec<serde_json::Value>,
    #[serde(default)]
    pub links: Option<Links>,
}

/// Document links.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Links {
    #[serde(default)]
    pub next: Option<String>,
}

/// One resource object.
#[derive(Debug, Clone, Deserialize)]
pub struct Resource<A> {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default = "Option::default")]
    pub attributes: Option<A>,
    #[serde(default)]
    pub relationships: BTreeMap<String, Relationship>,
}

/// A relationship: one linkage, many, or only links.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Relationship {
    #[serde(default)]
    pub data: Option<Linkage>,
}

/// Relationship linkage.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Linkage {
    Many(Vec<ResourceId>),
    One(ResourceId),
}

impl Linkage {
    pub fn ids(&self) -> Vec<&ResourceId> {
        match self {
            Self::Many(v) => v.iter().collect(),
            Self::One(r) => vec![r],
        }
    }
}

/// `{type, id}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResourceId {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

impl<A> Resource<A> {
    /// The IDs linked under relationship `name`.
    pub fn related_ids(&self, name: &str) -> Vec<String> {
        self.relationships
            .get(name)
            .and_then(|r| r.data.as_ref())
            .map(|d| d.ids().into_iter().map(|i| i.id.clone()).collect())
            .unwrap_or_default()
    }
}

macro_rules! wire_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident),* $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        pub enum $name { $($variant),* }
    };
}

wire_enum!(
    /// `CertificateType` (the values Russet distinguishes; others stay [`Open::Unknown`]).
    CertificateType {
        DEVELOPMENT, DISTRIBUTION, IOS_DEVELOPMENT, IOS_DISTRIBUTION, MAC_APP_DEVELOPMENT, MAC_APP_DISTRIBUTION,
        MAC_INSTALLER_DISTRIBUTION, DEVELOPER_ID_APPLICATION, DEVELOPER_ID_APPLICATION_G2, DEVELOPER_ID_KEXT,
        DEVELOPER_ID_KEXT_G2, PASS_TYPE_ID, PASS_TYPE_ID_WITH_NFC,
    }
);
wire_enum!(
    /// `BundleIdPlatform`.
    BundleIdPlatform { IOS, MAC_OS, UNIVERSAL, SERVICES }
);
wire_enum!(
    /// `DeviceClass`.
    DeviceClass { APPLE_WATCH, IPAD, IPHONE, IPOD, APPLE_TV, MAC, APPLE_VISION_PRO }
);
wire_enum!(
    /// Device `status`.
    DeviceStatus { ENABLED, DISABLED }
);
wire_enum!(
    /// `ProfileType`.
    ProfileType {
        IOS_APP_DEVELOPMENT, IOS_APP_STORE, IOS_APP_ADHOC, IOS_APP_INHOUSE, MAC_APP_DEVELOPMENT, MAC_APP_STORE,
        MAC_APP_DIRECT, TVOS_APP_DEVELOPMENT, TVOS_APP_STORE, TVOS_APP_ADHOC, TVOS_APP_INHOUSE,
        MAC_CATALYST_APP_DEVELOPMENT, MAC_CATALYST_APP_STORE, MAC_CATALYST_APP_DIRECT,
    }
);
wire_enum!(
    /// `profileState`.
    ProfileState { ACTIVE, INVALID }
);

/// `certificates` attributes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateAttributes {
    #[serde(default)]
    pub name: Option<String>,
    pub certificate_type: Open<CertificateType>,
    #[serde(default)]
    pub display_name: Option<String>,
    /// Uppercase hex without leading zeros.
    #[serde(default)]
    pub serial_number: Option<String>,
    #[serde(default)]
    pub platform: Option<Open<BundleIdPlatform>>,
    #[serde(default)]
    pub expiration_date: Option<String>,
    /// Base64 DER.
    #[serde(default)]
    pub certificate_content: Option<String>,
    #[serde(default)]
    pub activated: Option<bool>,
}

/// `devices` attributes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAttributes {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub platform: Option<Open<BundleIdPlatform>>,
    pub udid: String,
    #[serde(default)]
    pub device_class: Option<Open<DeviceClass>>,
    #[serde(default)]
    pub status: Option<Open<DeviceStatus>>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub added_date: Option<String>,
}

/// `bundleIds` attributes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleIdAttributes {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub platform: Option<Open<BundleIdPlatform>>,
    pub identifier: String,
    /// The App ID prefix (usually the team ID).
    #[serde(default)]
    pub seed_id: Option<String>,
}

/// `bundleIdCapabilities` attributes. Capability types are open strings [V5].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleIdCapabilityAttributes {
    #[serde(default)]
    pub capability_type: Option<String>,
    #[serde(default)]
    pub settings: Option<serde_json::Value>,
}

/// `profiles` attributes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAttributes {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub platform: Option<Open<BundleIdPlatform>>,
    pub profile_type: Open<ProfileType>,
    #[serde(default)]
    pub profile_state: Option<Open<ProfileState>>,
    /// Base64 CMS (`.mobileprovision`).
    #[serde(default)]
    pub profile_content: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub created_date: Option<String>,
    #[serde(default)]
    pub expiration_date: Option<String>,
}

/// `apps` attributes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppAttributes {
    #[serde(default)]
    pub name: Option<String>,
    pub bundle_id: String,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub primary_locale: Option<String>,
}

/// `builds` attributes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildAttributes {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub uploaded_date: Option<String>,
    #[serde(default)]
    pub expiration_date: Option<String>,
    #[serde(default)]
    pub expired: Option<bool>,
    #[serde(default)]
    pub processing_state: Option<String>,
}

/// `betaGroups` attributes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BetaGroupAttributes {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub is_internal_group: Option<bool>,
    #[serde(default)]
    pub public_link_enabled: Option<bool>,
}

/// `ciProducts` attributes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CiProductAttributes {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub product_type: Option<String>,
}

/// A bundle ID with its capabilities (from `include=bundleIdCapabilities`).
#[derive(Debug, Clone)]
pub struct BundleIdRecord {
    pub resource: Resource<BundleIdAttributes>,
    pub capabilities: Vec<Resource<BundleIdCapabilityAttributes>>,
}

impl BundleIdRecord {
    pub fn identifier(&self) -> &str {
        self.resource.attributes.as_ref().map_or("", |a| a.identifier.as_str())
    }

    /// The capability types enabled on the bundle ID.
    pub fn capability_types(&self) -> Vec<String> {
        self.capabilities.iter().filter_map(|c| c.attributes.as_ref()?.capability_type.clone()).collect()
    }
}
