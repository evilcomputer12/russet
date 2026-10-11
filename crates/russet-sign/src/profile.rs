// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Provisioning profiles (SIGN §8.3): a CMS `SignedData` whose encapsulated content is a property list.

use std::path::Path;
use std::time::{Duration, SystemTime};

use cryptographic_message_syntax::SignedData;
use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::util::{glob_match, io_error, plist_dict_from_bytes};

/// What a profile is for, derived from its content (SIGN §8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileKind {
    /// `get-task-allow` and a device list (`IOS_APP_DEVELOPMENT`).
    Development,
    /// A device list without `get-task-allow` (`IOS_APP_ADHOC`).
    AdHoc,
    /// Neither devices nor `ProvisionsAllDevices` (`IOS_APP_STORE`).
    AppStore,
    /// `ProvisionsAllDevices` (`IOS_APP_INHOUSE`).
    InHouse,
}

impl ProfileKind {
    /// `development`, `ad-hoc`, `app-store`, `in-house`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::AdHoc => "ad-hoc",
            Self::AppStore => "app-store",
            Self::InHouse => "in-house",
        }
    }
}

/// A parsed provisioning profile (the CMS payload).
#[derive(Debug, Clone)]
pub struct ProvisioningProfile {
    pub uuid: String,
    pub name: String,
    /// `TeamIdentifier[0]`.
    pub team_id: String,
    pub team_name: String,
    /// `ApplicationIdentifierPrefix[0]` (the App ID prefix, normally the team ID).
    pub app_id_prefix: String,
    /// `Entitlements.application-identifier`: `<PREFIX>.<bundle id>` or `<PREFIX>.*`.
    pub application_identifier: String,
    pub kind: ProfileKind,
    /// For example `["iOS", "xrOS", "visionOS"]`.
    pub platforms: Vec<String>,
    pub entitlements: plist::Dictionary,
    /// DER certificates allowed to sign with this profile.
    pub developer_certificates: Vec<Vec<u8>>,
    pub provisioned_devices: Option<Vec<String>>,
    pub provisions_all_devices: bool,
    pub creation: SystemTime,
    pub expiration: SystemTime,
    pub ppq_check: Option<bool>,
    /// Whether `DER-Encoded-Profile` is present.
    pub has_der_profile: bool,
    /// The CMS bytes as read (what `embedded.mobileprovision` must contain).
    pub raw: Vec<u8>,
}

impl ProvisioningProfile {
    /// Parses a profile from its CMS bytes (`.mobileprovision`). The CMS signature is not checked here: the
    /// device does that against Apple's roots, and the portal is the source of truth for planning.
    pub fn parse(der: &[u8]) -> Result<Self> {
        let invalid = |why: String| {
            Error::new(codes::sign::PROFILE_INVALID, format!("invalid provisioning profile: {why}"))
                .with_hint("Download the profile again (`russet provision`), or pass a .mobileprovision file.")
        };
        let signed = SignedData::parse_ber(der).map_err(|e| invalid(format!("not CMS signed data ({e})")))?;
        let content = signed.signed_content().ok_or_else(|| invalid("the CMS carries no content".into()))?;
        let dict = plist_dict_from_bytes(content, "the profile payload").map_err(|e| invalid(e.message().into()))?;

        let string = |k: &str| dict.get(k).and_then(plist::Value::as_string).map(str::to_owned);
        let first_string = |k: &str| {
            dict.get(k)
                .and_then(plist::Value::as_array)
                .and_then(|a| a.first())
                .and_then(plist::Value::as_string)
                .map(str::to_owned)
        };
        let strings = |k: &str| {
            dict.get(k)
                .and_then(plist::Value::as_array)
                .map(|a| a.iter().filter_map(plist::Value::as_string).map(str::to_owned).collect::<Vec<_>>())
        };
        let date = |k: &str| dict.get(k).and_then(plist::Value::as_date).map(SystemTime::from);

        let uuid = string("UUID").ok_or_else(|| invalid("no UUID".into()))?;
        let team_id = first_string("TeamIdentifier").ok_or_else(|| invalid("no TeamIdentifier".into()))?;
        let entitlements = dict
            .get("Entitlements")
            .and_then(plist::Value::as_dictionary)
            .cloned()
            .ok_or_else(|| invalid("no Entitlements".into()))?;
        let application_identifier = entitlements
            .get("application-identifier")
            .and_then(plist::Value::as_string)
            .map(str::to_owned)
            .ok_or_else(|| invalid("no application-identifier entitlement".into()))?;
        let app_id_prefix = first_string("ApplicationIdentifierPrefix")
            .or_else(|| application_identifier.split_once('.').map(|(p, _)| p.to_owned()))
            .unwrap_or_else(|| team_id.clone());
        let developer_certificates = dict
            .get("DeveloperCertificates")
            .and_then(plist::Value::as_array)
            .map(|a| a.iter().filter_map(plist::Value::as_data).map(<[u8]>::to_vec).collect())
            .unwrap_or_default();
        let provisioned_devices = strings("ProvisionedDevices");
        let provisions_all_devices =
            dict.get("ProvisionsAllDevices").and_then(plist::Value::as_boolean).unwrap_or(false);
        let get_task_allow = entitlements.get("get-task-allow").and_then(plist::Value::as_boolean).unwrap_or(false);
        let kind = if provisions_all_devices {
            ProfileKind::InHouse
        } else if provisioned_devices.is_some() {
            if get_task_allow { ProfileKind::Development } else { ProfileKind::AdHoc }
        } else {
            ProfileKind::AppStore
        };
        Ok(Self {
            uuid,
            name: string("Name").unwrap_or_default(),
            team_id,
            team_name: string("TeamName").unwrap_or_default(),
            app_id_prefix,
            application_identifier,
            kind,
            platforms: strings("Platform").unwrap_or_default(),
            entitlements,
            developer_certificates,
            provisioned_devices,
            provisions_all_devices,
            creation: date("CreationDate").unwrap_or(SystemTime::UNIX_EPOCH),
            expiration: date("ExpirationDate").ok_or_else(|| invalid("no ExpirationDate".into()))?,
            ppq_check: dict.get("PPQCheck").and_then(plist::Value::as_boolean),
            has_der_profile: dict.contains_key("DER-Encoded-Profile"),
            raw: der.to_vec(),
        })
    }

    /// Reads and parses a `.mobileprovision` file.
    pub fn open(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).map_err(|e| io_error("cannot read the profile", path, e))?;
        Self::parse(&data)
    }

    /// The bundle-ID part of `application-identifier` (`com.example.app`, `*` or `com.example.*`).
    pub fn bundle_id_pattern(&self) -> &str {
        let prefix = format!("{}.", self.app_id_prefix);
        self.application_identifier.strip_prefix(&prefix).unwrap_or_else(|| {
            self.application_identifier.split_once('.').map_or(self.application_identifier.as_str(), |(_, b)| b)
        })
    }

    /// Whether the profile's App ID is a wildcard.
    pub fn is_wildcard(&self) -> bool {
        self.bundle_id_pattern().contains('*')
    }

    /// Whether the profile belongs to `team` and its application identifier matches `bundle_id` exactly or
    /// through a wildcard.
    pub fn covers(&self, bundle_id: &str, team: &str) -> bool {
        self.team_id == team && glob_match(self.bundle_id_pattern(), bundle_id)
    }

    /// Whether `der` is one of the profile's developer certificates.
    pub fn has_certificate(&self, der: &[u8]) -> bool {
        self.developer_certificates.iter().any(|c| c.as_slice() == der)
    }

    /// Whether the profile lets `udid` install the app (case-insensitive; always for in-house profiles).
    pub fn has_device(&self, udid: &str) -> bool {
        self.provisions_all_devices
            || self.provisioned_devices.as_ref().is_some_and(|d| d.iter().any(|x| x.eq_ignore_ascii_case(udid)))
    }

    /// Whether the profile expires within `d` from now (or already has).
    pub fn expires_within(&self, d: Duration) -> bool {
        match SystemTime::now().checked_add(d) {
            Some(limit) => self.expiration <= limit,
            None => true,
        }
    }

    /// The `get-task-allow` entitlement of the profile.
    pub fn get_task_allow(&self) -> bool {
        self.entitlements.get("get-task-allow").and_then(plist::Value::as_boolean).unwrap_or(false)
    }
}
