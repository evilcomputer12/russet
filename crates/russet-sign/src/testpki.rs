// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Synthetic PKI for tests (feature `test-pki`; SIGN §18): self-signed development identities whose subject
//! carries a team ID like Apple's, and CMS-signed provisioning profiles. No Apple key material is involved, and
//! nothing here is meant to be trusted by a device.

use std::time::{Duration, SystemTime};

use plist::{Dictionary, Value};
use x509_certificate::{
    CapturedX509Certificate, EcdsaCurve, InMemorySigningKeyPair, KeyAlgorithm, X509CertificateBuilder,
};
use zeroize::Zeroizing;

use crate::identity::InMemoryIdentity;

/// A synthetic identity: certificate, private key (PKCS#8) and team.
#[derive(Debug, Clone)]
pub struct TestIdentity {
    pub cert_der: Vec<u8>,
    pub key_pkcs8: Zeroizing<Vec<u8>>,
    pub team_id: String,
}

impl TestIdentity {
    /// The identity as a [`InMemoryIdentity`].
    pub fn load(&self) -> InMemoryIdentity {
        InMemoryIdentity::from_pkcs8(self.cert_der.clone(), &self.key_pkcs8, None)
            .unwrap_or_else(|e| panic!("synthetic identity: {e}"))
    }
}

fn certificate(
    cn: &str,
    team: Option<&str>,
    serial: i64,
    days: i64,
    key: &InMemorySigningKeyPair,
) -> CapturedX509Certificate {
    let mut b = X509CertificateBuilder::default();
    let subject = b.subject();
    let _ = subject.append_common_name_utf8_string(cn);
    if let Some(team) = team {
        let _ = subject.append_organizational_unit_utf8_string(team);
    }
    let _ = subject.append_organization_utf8_string("Russet Test");
    let _ = subject.append_country_utf8_string("US");
    b.serial_number(serial);
    b.validity_duration(chrono::Duration::days(days));
    b.constraint_not_ca();
    b.create_with_key_pair(key).unwrap_or_else(|e| panic!("synthetic certificate: {e}"))
}

fn ecdsa_key() -> InMemorySigningKeyPair {
    InMemorySigningKeyPair::generate_random(KeyAlgorithm::Ecdsa(EcdsaCurve::Secp256r1))
        .unwrap_or_else(|e| panic!("synthetic key: {e}"))
}

/// A synthetic development identity (`Apple Development: Russet Test (<team>)`, OU = team), ECDSA P-256.
pub fn development_identity(team: &str) -> TestIdentity {
    identity_with_key(team, &format!("Apple Development: Russet Test ({team})"), ecdsa_key(), 365)
}

/// A synthetic identity from a given PKCS#8 key (RSA keys must come from the caller: ring cannot generate
/// them). The certificate is valid for `days` days.
pub fn identity_from_pkcs8(team: &str, cn: &str, key_pkcs8: &[u8], days: i64) -> TestIdentity {
    let key = InMemorySigningKeyPair::from_pkcs8_der(key_pkcs8).unwrap_or_else(|e| panic!("synthetic key: {e}"));
    identity_with_key(team, cn, key, days)
}

fn identity_with_key(team: &str, cn: &str, key: InMemorySigningKeyPair, days: i64) -> TestIdentity {
    static SERIAL: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0x1000);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let cert = certificate(cn, Some(team), serial, days, &key);
    TestIdentity {
        cert_der: cert.constructed_data().to_vec(),
        key_pkcs8: key.to_pkcs8_one_asymmetric_key_der(),
        team_id: team.into(),
    }
}

/// Builds CMS-signed provisioning profiles.
#[derive(Debug, Clone)]
pub struct ProfileBuilder {
    pub uuid: String,
    pub name: String,
    pub team_id: String,
    pub app_id_prefix: String,
    /// The bundle part of `application-identifier` (`com.example.app`, `*`, `com.example.*`).
    pub bundle_pattern: String,
    pub entitlements: Dictionary,
    pub certificates: Vec<Vec<u8>>,
    pub devices: Option<Vec<String>>,
    pub provisions_all_devices: bool,
    pub created: SystemTime,
    pub expires: SystemTime,
    pub ppq_check: Option<bool>,
    pub der_profile: bool,
}

impl ProfileBuilder {
    /// A development profile: `get-task-allow`, `keychain-access-groups = [<TEAM>.*]`, the team identifier, one
    /// year of validity, an empty device list.
    pub fn development(team: &str, bundle_pattern: &str) -> Self {
        let mut e = Dictionary::new();
        e.insert("application-identifier".into(), Value::String(format!("{team}.{bundle_pattern}")));
        e.insert("com.apple.developer.team-identifier".into(), Value::String(team.into()));
        e.insert("get-task-allow".into(), Value::Boolean(true));
        e.insert(
            "keychain-access-groups".into(),
            Value::Array(vec![Value::String(format!("{team}.*")), Value::String("com.apple.token".into())]),
        );
        let now = SystemTime::now();
        Self {
            uuid: uuid_like(),
            name: format!("Russet Development {bundle_pattern}"),
            team_id: team.into(),
            app_id_prefix: team.into(),
            bundle_pattern: bundle_pattern.into(),
            entitlements: e,
            certificates: Vec::new(),
            devices: Some(Vec::new()),
            provisions_all_devices: false,
            created: now,
            expires: now + Duration::from_secs(365 * 24 * 3600),
            ppq_check: Some(true),
            der_profile: true,
        }
    }

    /// An App Store profile: no devices, no `get-task-allow`, `beta-reports-active`.
    pub fn app_store(team: &str, bundle_pattern: &str) -> Self {
        let mut p = Self::development(team, bundle_pattern);
        p.name = format!("Russet App Store {bundle_pattern}");
        p.entitlements.insert("get-task-allow".into(), Value::Boolean(false));
        p.entitlements.insert("beta-reports-active".into(), Value::Boolean(true));
        p.devices = None;
        p
    }

    /// An ad hoc profile: devices but no `get-task-allow`.
    pub fn ad_hoc(team: &str, bundle_pattern: &str) -> Self {
        let mut p = Self::development(team, bundle_pattern);
        p.name = format!("Russet Ad Hoc {bundle_pattern}");
        p.entitlements.insert("get-task-allow".into(), Value::Boolean(false));
        p
    }

    pub fn certificate(mut self, der: &[u8]) -> Self {
        self.certificates.push(der.to_vec());
        self
    }

    pub fn device(mut self, udid: &str) -> Self {
        self.devices.get_or_insert_with(Vec::new).push(udid.into());
        self
    }

    pub fn entitlement(mut self, key: &str, value: Value) -> Self {
        self.entitlements.insert(key.into(), value);
        self
    }

    pub fn without_entitlement(mut self, key: &str) -> Self {
        self.entitlements.remove(key);
        self
    }

    pub fn expires_in(mut self, d: Duration) -> Self {
        self.expires = SystemTime::now() + d;
        self
    }

    pub fn expired(mut self) -> Self {
        self.expires = SystemTime::now() - Duration::from_secs(3600);
        self
    }

    pub fn uuid(mut self, uuid: &str) -> Self {
        self.uuid = uuid.into();
        self
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = name.into();
        self
    }

    /// The profile payload (an XML plist).
    pub fn payload(&self) -> Vec<u8> {
        let mut d = Dictionary::new();
        d.insert("AppIDName".into(), Value::String(format!("Russet {}", self.bundle_pattern)));
        d.insert("ApplicationIdentifierPrefix".into(), Value::Array(vec![Value::String(self.app_id_prefix.clone())]));
        d.insert("CreationDate".into(), Value::Date(self.created.into()));
        d.insert(
            "Platform".into(),
            Value::Array(vec![
                Value::String("iOS".into()),
                Value::String("xrOS".into()),
                Value::String("visionOS".into()),
            ]),
        );
        if let Some(ppq) = self.ppq_check {
            d.insert("PPQCheck".into(), Value::Boolean(ppq));
        }
        if self.der_profile {
            d.insert("DER-Encoded-Profile".into(), Value::Data(vec![0x30, 0x00]));
        }
        d.insert(
            "DeveloperCertificates".into(),
            Value::Array(self.certificates.iter().map(|c| Value::Data(c.clone())).collect()),
        );
        d.insert("Entitlements".into(), Value::Dictionary(self.entitlements.clone()));
        d.insert("ExpirationDate".into(), Value::Date(self.expires.into()));
        d.insert("Name".into(), Value::String(self.name.clone()));
        if let Some(devs) = &self.devices {
            d.insert(
                "ProvisionedDevices".into(),
                Value::Array(devs.iter().map(|u| Value::String(u.clone())).collect()),
            );
        }
        if self.provisions_all_devices {
            d.insert("ProvisionsAllDevices".into(), Value::Boolean(true));
        }
        d.insert("TeamIdentifier".into(), Value::Array(vec![Value::String(self.team_id.clone())]));
        d.insert("TeamName".into(), Value::String("Russet Test Team".into()));
        d.insert("TimeToLive".into(), Value::Integer(365.into()));
        d.insert("UUID".into(), Value::String(self.uuid.clone()));
        d.insert("Version".into(), Value::Integer(1.into()));
        let mut buf = Vec::new();
        Value::Dictionary(d).to_writer_xml(&mut buf).unwrap_or_else(|e| panic!("profile plist: {e}"));
        buf
    }

    /// The CMS-signed profile (`.mobileprovision` bytes), signed by a throwaway "profile signing" key.
    pub fn build(&self) -> Vec<u8> {
        let key = ecdsa_key();
        let cert = certificate("Russet Test Provisioning Profile Signing", None, 1, 30, &key);
        cryptographic_message_syntax::SignedDataBuilder::default()
            .content_inline(self.payload())
            .signer(cryptographic_message_syntax::SignerBuilder::new(&key, cert.clone()))
            .certificate(cert)
            .build_der()
            .unwrap_or_else(|e| panic!("profile CMS: {e}"))
    }
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    format!(
        "{:08X}-{:04X}-4{:03X}-8{:03X}-{:012X}",
        (t >> 32) as u32,
        (t >> 16) as u16,
        n & 0xfff,
        (t >> 4) & 0xfff,
        (t & 0xffff_ffff_ffff) ^ n
    )
}
