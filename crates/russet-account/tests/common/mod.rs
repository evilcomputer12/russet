// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Test support: a memory secret store, a temporary RUSSET_HOME, a synthetic xtool store, and a wiremock App
//! Store Connect fake that checks every request is a GET carrying a token scoped to exactly that request.

#![allow(dead_code, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use p256::pkcs8::EncodePrivateKey as _;
use russet_core::dirs::RussetDirs;
use russet_core::net::Endpoints;
use russet_secrets::{BackendInfo, BackendKind, SecretBytes, SecretId, SecretKind, SecretMeta, SecretStore};
use russet_sign::testpki::{ProfileBuilder, TestIdentity, identity_from_pkcs8};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::any};

pub const KEY_ID: &str = "2X9R4HXF34";
pub const ISSUER: &str = "69a6de7d-1111-2222-3333-444455556666";
pub const PREFIX: &str = "69A6DE7D";
pub const TEAM: &str = "ABCDE12345";
pub const UDID: &str = "00008101-000A1B2C3D4E5F60";

/// An in-memory secret store (the role of `RUSSET_SECRETS_BACKEND=memory`, which W3 provides).
#[derive(Default)]
pub struct MemorySecrets {
    pub items: Mutex<BTreeMap<String, (String, Vec<u8>)>>,
}

#[async_trait]
impl SecretStore for MemorySecrets {
    async fn put(&self, id: &SecretId, label: &str, v: SecretBytes) -> russet_core::Result<SecretMeta> {
        let size = v.len();
        self.items.lock().unwrap().insert(id.to_string(), (label.into(), v.expose().to_vec()));
        Ok(SecretMeta {
            id: id.clone(),
            label: label.into(),
            backend: BackendKind::Memory,
            created_at: time::OffsetDateTime::now_utc(),
            size_bytes: size,
        })
    }
    async fn get(&self, id: &SecretId) -> russet_core::Result<Option<SecretBytes>> {
        Ok(self.items.lock().unwrap().get(&id.to_string()).map(|(_, v)| SecretBytes::new(v.clone())))
    }
    async fn delete(&self, id: &SecretId) -> russet_core::Result<bool> {
        Ok(self.items.lock().unwrap().remove(&id.to_string()).is_some())
    }
    async fn list(&self, _kind: Option<SecretKind>) -> russet_core::Result<Vec<SecretMeta>> {
        Ok(Vec::new())
    }
    fn backend(&self) -> BackendInfo {
        BackendInfo { kind: BackendKind::Memory, available: true, locked: false, reason: None }
    }
}

/// A temporary RUSSET_HOME and an xtool data directory.
pub struct Env {
    pub tmp: tempfile::TempDir,
    pub dirs: RussetDirs,
    pub secrets: Arc<MemorySecrets>,
    pub xtool: PathBuf,
}

pub fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = RussetDirs::under(&tmp.path().join("home"));
    let xtool = tmp.path().join("xtool-data");
    std::fs::create_dir_all(&xtool).unwrap();
    Env { tmp, dirs, secrets: Arc::new(MemorySecrets::default()), xtool }
}

pub fn endpoints(asc: &str) -> Endpoints {
    let u = |s: &str| url::Url::parse(s).unwrap();
    Endpoints {
        asc: u(asc),
        gsa: u("http://127.0.0.1:9/gsa/"),
        tss: u("http://127.0.0.1:9/tss/"),
        anisette_libs: u("http://127.0.0.1:9/anisette/"),
        github: u("http://127.0.0.1:9/github/"),
        flutter_storage: u("http://127.0.0.1:9/flutter/"),
    }
}

/// The test App Store Connect key (P-256 PKCS#8 PEM).
pub fn p8_pem() -> String {
    let key = p256::ecdsa::SigningKey::from_slice(&[0x22; 32]).unwrap();
    key.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap().to_string()
}

/// A synthetic RSA-2048 development identity, as Apple issues them, and its key as PKCS#1 PEM (what xtool
/// stores). Generated once per test binary (about 3 s in debug builds).
pub fn rsa_identity() -> &'static (TestIdentity, String) {
    static ID: OnceLock<(TestIdentity, String)> = OnceLock::new();
    ID.get_or_init(|| {
        use rsa::pkcs1::EncodeRsaPrivateKey as _;
        use rsa::pkcs8::EncodePrivateKey as _;
        let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        let pkcs8 = key.to_pkcs8_der().unwrap();
        let id = identity_from_pkcs8(TEAM, &format!("Apple Development: Russet Test ({TEAM})"), pkcs8.as_bytes(), 365);
        let pem = key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap().to_string();
        (id, pem)
    })
}

/// Writes xtool's store: `XTLAuthToken` (App Store Connect login) and the identity file named after the issuer.
pub fn write_xtool(dir: &Path, with_identity: bool) {
    let token = json!({"appStoreConnect": {"_0": {"id": KEY_ID, "issuerID": ISSUER, "pem": p8_pem()}}});
    std::fs::write(dir.join("XTLAuthToken"), serde_json::to_vec(&token).unwrap()).unwrap();
    if with_identity {
        let (id, pem) = rsa_identity();
        let mut pk = plist::Dictionary::new();
        pk.insert("data".into(), plist::Value::Data(pem.as_bytes().to_vec()));
        let mut d = plist::Dictionary::new();
        d.insert("privateKey".into(), plist::Value::Dictionary(pk));
        d.insert("certificate".into(), plist::Value::Data(id.cert_der.clone()));
        plist::Value::Dictionary(d).to_file_binary(dir.join(ISSUER)).unwrap();
    }
    set_mode(&dir.join("XTLAuthToken"), 0o600);
    if with_identity {
        set_mode(&dir.join(ISSUER), 0o600);
    }
}

#[cfg(unix)]
pub fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
pub fn set_mode(_p: &Path, _mode: u32) {}

pub fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// One profile the fake serves.
#[derive(Clone)]
pub struct FakeProfile {
    pub id: String,
    pub bundle_resource: String,
    pub name: String,
    pub profile_type: String,
    pub content: Vec<u8>,
}

/// The fake's account state.
#[derive(Clone, Default)]
pub struct FakeState {
    /// `(id, certificate DER)` of DEVELOPMENT certificates.
    pub certificates: Vec<(String, Vec<u8>)>,
    pub devices: Vec<String>,
    /// `(resource id, identifier, capability types)`.
    pub bundle_ids: Vec<(String, String, Vec<String>)>,
    pub profiles: Vec<FakeProfile>,
    /// Paths answered 403 (role tests).
    pub forbidden: Vec<String>,
    /// The issuer whose tokens are accepted.
    pub issuer: String,
    /// Accept individual-key tokens (`sub: "user"`, no issuer) instead, and refuse them the provisioning
    /// endpoints, as App Store Connect does (SIGN §5.2).
    pub individual: bool,
}

pub struct FakeAsc {
    pub state: Mutex<FakeState>,
    pub requests: Mutex<Vec<(String, String)>>,
}

impl FakeAsc {
    pub fn non_get_requests(&self) -> usize {
        self.requests.lock().unwrap().iter().filter(|(m, _)| m != "GET").count()
    }
    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    pub fn paths(&self) -> Vec<String> {
        self.requests.lock().unwrap().iter().map(|(_, p)| p.clone()).collect()
    }
}

fn err(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({"errors": [{"status": status.to_string(), "code": code}]}))
}

struct Responder(Arc<FakeAsc>);

impl Respond for Responder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.0.requests.lock().unwrap().push((req.method.to_string(), req.url.path().to_owned()));
        let state = self.0.state.lock().unwrap().clone();
        // Every request must carry a token scoped to exactly itself (Wave 1's per-request GET scopes).
        let token =
            req.headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
        let Some(claims) = token.and_then(russet_asc::unverified_claims) else { return err(401, "NOT_AUTHORIZED") };
        let authorized = if state.individual {
            claims["sub"] == "user" && claims.get("iss").is_none()
        } else {
            claims["iss"] == state.issuer.as_str()
        };
        if !authorized {
            return err(401, "NOT_AUTHORIZED");
        }
        let want = match req.url.query() {
            Some(q) => russet_asc::ScopeEntry::get(&format!("{}?{q}", req.url.path())).0,
            None => format!("GET {}", req.url.path()),
        };
        let scoped = claims["scope"].as_array().is_some_and(|s| s.iter().any(|e| e.as_str() == Some(want.as_str())));
        if !scoped {
            return err(403, "FORBIDDEN.REQUEST_DOES_NOT_MATCH_SCOPE");
        }
        if req.method.as_str() != "GET" {
            return err(405, "METHOD_NOT_ALLOWED");
        }
        let path = req.url.path().to_owned();
        let provisioning = ["/v1/certificates", "/v1/devices", "/v1/bundleIds", "/v1/profiles"];
        if state.forbidden.contains(&path) || (state.individual && provisioning.iter().any(|p| path.starts_with(p))) {
            return err(403, "FORBIDDEN_ERROR");
        }
        let q = |k: &str| req.url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
        let profile_json = |p: &FakeProfile| {
            json!({"type": "profiles", "id": p.id, "attributes": {
                "name": p.name, "profileType": p.profile_type, "profileState": "ACTIVE", "uuid": format!("U-{}", p.id),
                "profileContent": b64(&p.content), "expirationDate": "2027-10-10T00:00:00.000+0000"}})
        };
        let ok = |v: Value| ResponseTemplate::new(200).set_body_json(v);
        match path.as_str() {
            "/v1/certificates" => {
                let data: Vec<Value> = state
                    .certificates
                    .iter()
                    .map(|(id, der)| {
                        let serial = russet_sign::certificate_info(der).unwrap().serial;
                        json!({"type": "certificates", "id": id, "attributes": {
                            "certificateType": "DEVELOPMENT", "serialNumber": serial, "certificateContent": b64(der),
                            "displayName": "Created via API"}})
                    })
                    .collect();
                ok(json!({"data": data}))
            }
            "/v1/devices" => ok(json!({"data": state.devices.iter().enumerate().map(|(i, u)| json!({
                "type": "devices", "id": format!("D{i}"), "attributes": {"udid": u, "platform": "IOS", "status": "ENABLED", "deviceClass": "IPHONE"}
            })).collect::<Vec<_>>()})),
            "/v1/bundleIds" => {
                let prefix = q("filter[identifier]").unwrap_or_default();
                let matching: Vec<_> =
                    state.bundle_ids.iter().filter(|(_, ident, _)| ident.starts_with(&prefix)).collect();
                let data: Vec<Value> = matching
                    .iter()
                    .map(|(id, ident, caps)| json!({"type": "bundleIds", "id": id,
                        "attributes": {"identifier": ident, "platform": "IOS", "seedId": TEAM},
                        "relationships": {"bundleIdCapabilities": {"data": caps.iter().map(|c| json!({"type": "bundleIdCapabilities", "id": format!("{id}-{c}")})).collect::<Vec<_>>()}}}))
                    .collect();
                let included: Vec<Value> = matching
                    .iter()
                    .flat_map(|(id, _, caps)| caps.iter().map(move |c| json!({"type": "bundleIdCapabilities", "id": format!("{id}-{c}"), "attributes": {"capabilityType": c}})))
                    .collect();
                ok(json!({"data": data, "included": included}))
            }
            "/v1/profiles" | "/v1/builds" | "/v1/apps" => ok(json!({"data": []})),
            p if p.starts_with("/v1/bundleIds/") && p.ends_with("/profiles") => {
                let bid = p.trim_start_matches("/v1/bundleIds/").trim_end_matches("/profiles");
                ok(
                    json!({"data": state.profiles.iter().filter(|x| x.bundle_resource == bid).map(profile_json).collect::<Vec<_>>()}),
                )
            }
            p if p.starts_with("/v1/profiles/") => {
                let id = p.trim_start_matches("/v1/profiles/");
                match state.profiles.iter().find(|x| x.id == id) {
                    Some(x) => ok(json!({"data": profile_json(x)})),
                    None => err(404, "NOT_FOUND"),
                }
            }
            _ => err(404, "NOT_FOUND"),
        }
    }
}

pub async fn fake_asc(state: FakeState) -> (MockServer, Arc<FakeAsc>) {
    let server = MockServer::start().await;
    let fake = Arc::new(FakeAsc { state: Mutex::new(state), requests: Mutex::new(Vec::new()) });
    Mock::given(any()).respond_with(Responder(fake.clone())).mount(&server).await;
    (server, fake)
}

/// The xtool-style development profile for `bundle` that lists the RSA identity and the test device.
pub fn xtl_profile(bundle: &str) -> Vec<u8> {
    let (id, _) = rsa_identity();
    ProfileBuilder::development(TEAM, bundle)
        .name(&format!("XTL {bundle}"))
        .certificate(&id.cert_der)
        .device(UDID)
        .build()
}

/// The state of a team where xtool already registered `bundle` and made its profile.
pub fn xtool_team(bundle: &str) -> FakeState {
    let (id, _) = rsa_identity();
    FakeState {
        certificates: vec![("CERT1".into(), id.cert_der.clone())],
        devices: vec![UDID.into()],
        bundle_ids: vec![("BID1".into(), bundle.into(), vec![])],
        profiles: vec![FakeProfile {
            id: "PROF1".into(),
            bundle_resource: "BID1".into(),
            name: format!("XTL {bundle}"),
            profile_type: "IOS_APP_DEVELOPMENT".into(),
            content: xtl_profile(bundle),
        }],
        forbidden: Vec::new(),
        issuer: ISSUER.into(),
        individual: false,
    }
}

/// A project directory: `russet.toml` with `[app] bundle-id`, plus an optional `russet.local.toml`.
pub fn project(dir: &Path, bundle_id: &str, local: Option<&str>) -> PathBuf {
    let p = dir.join("project");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("russet.toml"), format!("schema = 1\n[app]\nbundle-id = \"{bundle_id}\"\n")).unwrap();
    if let Some(l) = local {
        std::fs::write(p.join("russet.local.toml"), l).unwrap();
    }
    p
}
