// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The App Store Connect client against a wiremock fake that enforces Apple's token rules [V3]: the signature,
//! `aud`, a 20-minute lifetime cap (401 NOT_AUTHORIZED) and scope matching (403
//! FORBIDDEN.REQUEST_DOES_NOT_MATCH_SCOPE, query order ignored, `limit`/`cursor`/`sort` ignored).

#![allow(clippy::unwrap_used, clippy::result_large_err)]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::pkcs8::EncodePrivateKey as _;
use russet_asc::model::{CertificateType, Open};
use russet_asc::*;
use russet_core::error::codes;
use secrecy::ExposeSecret as _;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::method};

const KEY_ID: &str = "2X9R4HXF34";
const ISSUER: &str = "69a6de7d-1111-2222-3333-444455556666";

fn signing_key() -> SigningKey {
    SigningKey::from_slice(&[0x11; 32]).unwrap()
}

fn asc_key() -> AscKey {
    let pem = signing_key().to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap();
    AscKey::from_p8(KEY_ID, Issuer::Team { issuer_id: ISSUER.into() }, pem.as_bytes()).unwrap()
}

/// What an Apple scope check compares: the method and path, plus the query without ignored parameters, as a set.
fn normalize(entry: &str) -> (String, BTreeSet<String>) {
    let (head, query) = entry.split_once('?').unwrap_or((entry, ""));
    let params = query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter(|p| !matches!(p.split('=').next(), Some("limit" | "cursor" | "sort")))
        .map(str::to_owned)
        .collect();
    (head.to_owned(), params)
}

struct FakeAsc {
    verifying: VerifyingKey,
    hits: AtomicUsize,
    fail_first: Mutex<Vec<u16>>,
    seen: Mutex<Vec<String>>,
}

impl FakeAsc {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            verifying: *signing_key().verifying_key(),
            hits: AtomicUsize::new(0),
            fail_first: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn error(status: u16, code: &str) -> ResponseTemplate {
        ResponseTemplate::new(status)
            .set_body_json(json!({"errors": [{"status": status.to_string(), "code": code, "detail": "fake"}]}))
    }

    fn check_token(&self, req: &Request) -> Result<(), ResponseTemplate> {
        let auth = req.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
        let token = auth.strip_prefix("Bearer ").ok_or_else(|| Self::error(401, "NOT_AUTHORIZED"))?;
        let mut parts = token.split('.');
        let (h, c, s) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
        let sig = Signature::from_slice(&b64url_decode(s).unwrap()).map_err(|_| Self::error(401, "NOT_AUTHORIZED"))?;
        self.verifying.verify(format!("{h}.{c}").as_bytes(), &sig).map_err(|_| Self::error(401, "NOT_AUTHORIZED"))?;
        let header: Value = serde_json::from_slice(&b64url_decode(h).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], KEY_ID);
        let claims = unverified_claims(token).unwrap();
        assert_eq!(claims["aud"], "appstoreconnect-v1");
        assert_eq!(claims["iss"], ISSUER);
        let lifetime = claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap();
        if lifetime > 20 * 60 {
            return Err(Self::error(401, "NOT_AUTHORIZED"));
        }
        if let Some(scope) = claims.get("scope").and_then(Value::as_array) {
            let pq = match req.url.query() {
                Some(q) => format!("GET {}?{q}", req.url.path()),
                None => format!("GET {}", req.url.path()),
            };
            let want = normalize(&pq);
            if !scope.iter().filter_map(Value::as_str).any(|e| normalize(e) == want) {
                return Err(Self::error(403, "FORBIDDEN.REQUEST_DOES_NOT_MATCH_SCOPE"));
            }
        }
        Ok(())
    }

    fn body(&self, req: &Request, base: &str) -> ResponseTemplate {
        let path = req.url.path();
        let cursor = req.url.query_pairs().find(|(k, _)| k == "cursor").map(|(_, v)| v.into_owned());
        let limit = req.url.query_pairs().find(|(k, _)| k == "limit").map(|(_, v)| v.into_owned());
        let ok = |v: Value| {
            ResponseTemplate::new(200)
                .insert_header("x-rate-limit", "user-hour-lim:3600;user-hour-rem:3550;")
                .set_body_json(v)
        };
        match path {
            "/v1/certificates" if cursor.is_none() => ok(json!({
                "data": [cert("C1", "DEVELOPMENT", "1A2B")],
                // Apple's next links carry the cursor; this one drops `limit`, which the client must re-add.
                "links": {"next": format!("{base}v1/certificates?cursor=PAGE2")}
            })),
            "/v1/certificates" => {
                assert_eq!(limit.as_deref(), Some("200"), "the client keeps limit next to cursor");
                ok(json!({"data": [cert("C2", "NEW_KIND_FROM_THE_FUTURE", "FF")], "links": {}}))
            }
            "/v1/devices" => ok(json!({"data": [{"type": "devices", "id": "D1", "attributes": {
                "name": "iPhone", "platform": "IOS", "udid": "00008101-000A1B2C3D4E5F60", "deviceClass": "IPHONE", "status": "ENABLED"}}]})),
            "/v1/bundleIds" => ok(json!({
                "data": [
                    bundle("B1", "com.example.hello", &["CAP1"]),
                    bundle("B2", "com.example.hello.widget", &["CAP2"]),
                ],
                "included": [
                    {"type": "bundleIdCapabilities", "id": "CAP1", "attributes": {"capabilityType": "PUSH_NOTIFICATIONS"}},
                    {"type": "bundleIdCapabilities", "id": "CAP2", "attributes": {"capabilityType": "DRIVERKIT_PUBLIC"}}
                ]
            })),
            "/v1/profiles/P1" => ok(json!({"data": {"type": "profiles", "id": "P1", "attributes": {
                "name": "XTL profile", "profileType": "IOS_APP_DEVELOPMENT", "profileState": "ACTIVE", "uuid": "U-1",
                "profileContent": "AAEC"}}})),
            "/v1/apps" => ok(json!({"data": []})),
            "/v1/elsewhere" => ok(json!({"data": [], "links": {"next": "https://evil.example/v1/elsewhere?cursor=x"}})),
            _ => ResponseTemplate::new(404).set_body_json(json!({"errors": [{"status": "404", "code": "NOT_FOUND"}]})),
        }
    }
}

fn cert(id: &str, kind: &str, serial: &str) -> Value {
    json!({"type": "certificates", "id": id, "attributes": {"certificateType": kind, "serialNumber": serial, "displayName": "Created via API"}})
}

fn bundle(id: &str, identifier: &str, caps: &[&str]) -> Value {
    json!({"type": "bundleIds", "id": id,
        "attributes": {"identifier": identifier, "platform": "UNIVERSAL", "seedId": "ABCDE12345"},
        "relationships": {"bundleIdCapabilities": {"data": caps.iter().map(|c| json!({"type": "bundleIdCapabilities", "id": c})).collect::<Vec<_>>()}}})
}

struct Responder {
    fake: Arc<FakeAsc>,
    base: String,
}

impl Respond for Responder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.fake.hits.fetch_add(1, Ordering::SeqCst);
        self.fake.seen.lock().unwrap().push(req.url.path().to_owned());
        if let Some(status) = self.fake.fail_first.lock().unwrap().pop() {
            let t = FakeAsc::error(status, if status == 429 { "RATE_LIMIT_EXCEEDED" } else { "UNEXPECTED_ERROR" });
            return if status == 429 { t.insert_header("retry-after", "0") } else { t };
        }
        match self.fake.check_token(req) {
            Ok(()) => self.fake.body(req, &self.base),
            Err(t) => t,
        }
    }
}

async fn server() -> (MockServer, Arc<FakeAsc>) {
    let s = MockServer::start().await;
    let fake = FakeAsc::new();
    let base = format!("{}/", s.uri());
    Mock::given(method("GET")).respond_with(Responder { fake: fake.clone(), base }).mount(&s).await;
    (s, fake)
}

fn fast() -> RetryPolicy {
    RetryPolicy {
        base_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(5),
        low_quota_delay: Duration::ZERO,
        ..RetryPolicy::default()
    }
}

fn client(s: &MockServer, mode: TokenMode) -> AscClient {
    let base = url::Url::parse(&format!("{}/", s.uri())).unwrap();
    let auth = JwtAuth::new(base, Arc::new(TokenFactory::new(asc_key())), mode);
    AscClient::with_policy(Arc::new(auth), fast()).unwrap()
}

fn per_request() -> TokenMode {
    TokenMode::PerRequest { lifetime: Duration::from_secs(600) }
}

#[test]
fn jwts_carry_apple_claims_and_verify() {
    let key = asc_key();
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_700_000);
    let token = key.mint(now, Duration::from_secs(1140), Some(&["GET /v1/apps".into()])).unwrap();
    let t = token.expose_secret();
    let claims = unverified_claims(t).unwrap();
    assert_eq!(claims["iss"], ISSUER);
    assert_eq!(claims["iat"], 1_791_700_000u64);
    assert_eq!(claims["exp"], 1_791_701_140u64);
    assert_eq!(claims["aud"], "appstoreconnect-v1");
    assert_eq!(claims["scope"], json!(["GET /v1/apps"]));
    let (signed, sig) = t.rsplit_once('.').unwrap();
    let sig = Signature::from_slice(&b64url_decode(sig).unwrap()).unwrap();
    signing_key().verifying_key().verify(signed.as_bytes(), &sig).unwrap();

    let pem = signing_key().to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap();
    let individual = AscKey::from_p8(KEY_ID, Issuer::Individual, pem.as_bytes()).unwrap();
    assert!(individual.is_individual());
    let claims =
        unverified_claims(individual.mint(now, Duration::from_secs(60), None).unwrap().expose_secret()).unwrap();
    assert_eq!(claims["sub"], "user");
    assert!(claims.get("iss").is_none() && claims.get("scope").is_none());

    // Debug output never shows key material.
    assert!(!format!("{key:?}").contains("PRIVATE"));
    for bad in [&b"-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"[..], b"\xff\xfe", b""] {
        assert_eq!(AscKey::from_p8(KEY_ID, Issuer::Individual, bad).unwrap_err().code(), codes::account::KEY_INVALID);
    }
    assert_eq!(
        AscKey::from_p8("bad id!", Issuer::Individual, pem.as_bytes()).unwrap_err().code(),
        codes::core::INVALID_ARGUMENT
    );
}

struct ManualClock {
    start_wall: SystemTime,
    start: Instant,
    offset: Mutex<Duration>,
}

impl russet_core::time::Clock for ManualClock {
    fn now(&self) -> SystemTime {
        self.start_wall + *self.offset.lock().unwrap()
    }
    fn instant(&self) -> Instant {
        self.start + *self.offset.lock().unwrap()
    }
}

#[test]
fn bearer_tokens_are_cached_until_a_minute_remains() {
    let clock = Arc::new(ManualClock {
        start_wall: SystemTime::now(),
        start: Instant::now(),
        offset: Mutex::new(Duration::ZERO),
    });
    let f = TokenFactory::with_clock(asc_key(), clock.clone());
    let a = f.bearer().unwrap().expose_secret().to_owned();
    *clock.offset.lock().unwrap() = Duration::from_secs(17 * 60);
    assert_eq!(f.bearer().unwrap().expose_secret(), a, "reused within 18 minutes");
    *clock.offset.lock().unwrap() = Duration::from_secs(18 * 60 + 1);
    let b = f.bearer().unwrap().expose_secret().to_owned();
    assert_ne!(b, a, "replaced once less than a minute remains");
    let claims = unverified_claims(&b).unwrap();
    assert_eq!(claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(), 19 * 60);
    assert!(claims.get("scope").is_none());

    assert_eq!(
        f.scoped(&[ScopeEntry("POST /v1/certificates".into())], Duration::from_secs(60)).unwrap_err().code(),
        codes::core::INVALID_ARGUMENT,
        "Wave 1 mints GET scopes only"
    );
    assert!(f.scoped(&[], Duration::from_secs(60)).is_err());
}

#[tokio::test]
async fn reads_every_resource_with_per_request_scopes() {
    let (s, fake) = server().await;
    let c = client(&s, per_request());

    let certs = c.certificates().list(&CertificateFilter::default()).await.unwrap();
    assert_eq!(certs.len(), 2, "both pages");
    assert!(certs[0].attributes.as_ref().unwrap().certificate_type.is(&CertificateType::DEVELOPMENT));
    assert_eq!(
        certs[1].attributes.as_ref().unwrap().certificate_type,
        Open::Unknown("NEW_KIND_FROM_THE_FUTURE".into())
    );
    assert_eq!(c.quota(), Quota { limit: Some(3600), remaining: Some(3550) });

    let devices =
        c.devices().list(&DeviceFilter { status: Some("ENABLED".into()), ..Default::default() }).await.unwrap();
    assert_eq!(devices[0].attributes.as_ref().unwrap().udid, "00008101-000A1B2C3D4E5F60");

    // filter[identifier] is a prefix match; the exact helper keeps only the equal one.
    let found = c.bundle_ids().find_exact("com.example.hello").await.unwrap().unwrap();
    assert_eq!(found.resource.id, "B1");
    assert_eq!(found.capability_types(), vec!["PUSH_NOTIFICATIONS"]);
    let all = c.bundle_ids().list(Some("com.example.hello")).await.unwrap();
    assert_eq!(all[1].capability_types(), vec!["DRIVERKIT_PUBLIC"], "unknown capability types survive [V5]");
    assert!(c.bundle_ids().find_exact("com.example.nothing").await.unwrap().is_none());

    let p = c.profiles().get("P1").await.unwrap();
    assert_eq!(p.attributes.unwrap().profile_content.as_deref(), Some("AAEC"));
    assert!(c.apps().list(Some("com.example.hello")).await.unwrap().is_empty());
    assert_eq!(c.profiles().get("NOPE").await.unwrap_err().code(), codes::core::NOT_FOUND);
    assert_eq!(c.profiles().get("../x").await.unwrap_err().code(), codes::core::INVALID_ARGUMENT);
    assert!(fake.hits.load(Ordering::SeqCst) >= 8);
}

#[tokio::test]
async fn ambiguous_exact_matches_are_errors() {
    let s = MockServer::start().await;
    let dup = json!({"data": [bundle("B1", "com.example.hello", &[]), bundle("B9", "com.example.hello", &[])]});
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(dup)).mount(&s).await;
    let c = client(&s, per_request());
    assert_eq!(c.bundle_ids().find_exact("com.example.hello").await.unwrap_err().code(), codes::account::ASC_AMBIGUOUS);
}

#[tokio::test]
async fn a_scope_mismatch_is_account_asc_scope() {
    let (s, _) = server().await;
    let c = client(
        &s,
        TokenMode::Fixed { scope: vec![ScopeEntry::get("/v1/certificates")], lifetime: Duration::from_secs(600) },
    );
    let e = c.devices().list(&DeviceFilter::default()).await.unwrap_err();
    assert_eq!(e.code(), codes::account::ASC_SCOPE);
    assert!(e.hint().is_some());
    // The same token is fine for what it is scoped to.
    assert_eq!(c.certificates().probe(1).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_sixty_minute_token_is_account_asc_unauthorized() {
    let (s, _) = server().await;
    let c = client(
        &s,
        TokenMode::Fixed { scope: vec![ScopeEntry::get("/v1/certificates")], lifetime: Duration::from_secs(3600) },
    );
    let e = c.certificates().probe(1).await.unwrap_err();
    assert_eq!(e.code(), codes::account::ASC_UNAUTHORIZED);
    assert_eq!(e.code().exit_code(), 5);
}

#[tokio::test]
async fn retries_rate_limits_and_server_errors() {
    let (s, fake) = server().await;
    let c = client(&s, per_request());
    *fake.fail_first.lock().unwrap() = vec![429, 429];
    assert_eq!(c.certificates().probe(1).await.unwrap().len(), 1, "two 429s, then success");

    *fake.fail_first.lock().unwrap() = vec![503];
    assert!(c.devices().list(&DeviceFilter::default()).await.is_ok(), "one 503 is retried");

    *fake.fail_first.lock().unwrap() = vec![503, 503, 503];
    let e = c.devices().list(&DeviceFilter::default()).await.unwrap_err();
    assert_eq!(e.code(), codes::account::APPLE_SERVICE_UNAVAILABLE);
    assert!(e.to_data().retryable);

    *fake.fail_first.lock().unwrap() = vec![429; 10];
    assert_eq!(c.devices().list(&DeviceFilter::default()).await.unwrap_err().code(), codes::account::ASC_RATE_LIMITED);
}

#[tokio::test]
async fn next_links_never_leave_the_origin_and_raw_get_is_allowlisted() {
    let (s, fake) = server().await;
    let c = client(&s, per_request());
    let e = c.get_all::<serde_json::Value>("/v1/elsewhere").await.unwrap_err();
    assert!(e.message().contains("another host"));

    assert_eq!(c.raw_get("/v1/users").await.unwrap_err().code(), codes::perm::DENIED);
    assert_eq!(c.raw_get("/v1/apps/../users").await.unwrap_err().code(), codes::perm::DENIED);
    // Spellings the URL parser normalizes to /v1/users (url 2.5): refused before anything is sent.
    let before = fake.hits.load(Ordering::SeqCst);
    for sneaky in [
        "/v1/apps/%2e%2e/users",
        "/v1/apps/.%2e/users",
        "/v1/apps/%2E%2E/users",
        "/v1/apps\\%2e%2e\\users",
        "/v1/apps\\..\\users",
        "/v1/ciProducts/%2e%2e/%2e%2e/v1/users",
        "/v1/apps/%2f%2e%2e%2fusers",
        "/v1/apps/%5c..%5cusers",
        "/v1/apps/./../users",
        "/v1/apps//../users",
        "/v1/apps/x/..",
        "/v1/apps#/../users",
        "/v1/apps/./x",
        "/v1/apps/a b",
    ] {
        let e = c.raw_get(sneaky).await.unwrap_err();
        assert_eq!(e.code(), codes::perm::DENIED, "{sneaky}");
        assert!(c.raw_get_url(sneaky).is_err(), "{sneaky}");
    }
    assert_eq!(fake.hits.load(Ordering::SeqCst), before, "no refused path reached the server");
    let url = c.raw_get_url("/v1/apps?filter[bundleId]=com.example.hello").unwrap();
    assert_eq!((url.path(), url.query()), ("/v1/apps", Some("filter[bundleId]=com.example.hello")));
    assert!(c.raw_get_url("/v1/ciProducts/X1/workflows").is_ok());
    let (status, body) = c.raw_get("/v1/apps?filter[bundleId]=com.example.hello").await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(body["data"], json!([]));
    let (status, _) = c.raw_get("/v1/profiles/NOPE").await.unwrap();
    assert_eq!(status, 404, "raw_get returns errors as status and body");
}

#[tokio::test]
async fn the_doctor_check_skips_offline_and_reports_reachability() {
    use russet_doctor::{Check as _, CheckCx, Status};
    let (s, _) = server().await;
    let check = AscReachable::new(url::Url::parse(&format!("{}/", s.uri())).unwrap());
    assert_eq!(check.run(&CheckCx::default()).await.status, Status::Skip);
    let online = CheckCx { online: true, ..Default::default() };
    assert_eq!(check.run(&online).await.status, Status::Pass);
    let dead = AscReachable::new(url::Url::parse("http://127.0.0.1:9/").unwrap());
    assert_eq!(dead.run(&online).await.status, Status::Warn);
    let broken = AscReachable { base: Err("endpoints.json is not valid JSON".into()) };
    assert_eq!(broken.run(&online).await.status, Status::Fail);
}

/// The check reads the root the daemon uses: `asc` of `RUSSET_ENDPOINTS_FILE` (T3), else the public API.
#[test]
fn the_doctor_check_follows_the_endpoints_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("endpoints.json");
    assert_eq!(asc_base_from(None).unwrap().as_str(), DEFAULT_BASE_URL);
    std::fs::write(&file, r#"{"asc": "http://127.0.0.1:8080/", "gsa": "http://127.0.0.1:9/"}"#).unwrap();
    assert_eq!(asc_base_from(Some(&file)).unwrap().as_str(), "http://127.0.0.1:8080/");
    std::fs::write(&file, r#"{"github": "http://127.0.0.1:9/"}"#).unwrap();
    assert_eq!(asc_base_from(Some(&file)).unwrap().as_str(), DEFAULT_BASE_URL);
    std::fs::write(&file, "not json").unwrap();
    assert!(asc_base_from(Some(&file)).is_err());
    assert!(asc_base_from(Some(&dir.path().join("missing.json"))).is_err());
    assert!(is_loopback(&url::Url::parse("http://localhost:1/").unwrap()));
    assert!(!is_loopback(&url::Url::parse(DEFAULT_BASE_URL).unwrap()));
}

/// Every client of one key shares its limiter: the bucket and the reported quota span clients (and so calls).
#[tokio::test]
async fn clients_of_one_key_share_the_rate_limit() {
    let (s, _) = server().await;
    let limiter = Arc::new(RateLimiter::new(REQUESTS_PER_HOUR, 10));
    let make = || {
        let base = url::Url::parse(&format!("{}/", s.uri())).unwrap();
        let auth = JwtAuth::new(base, Arc::new(TokenFactory::new(asc_key())), per_request());
        AscClient::with_limiter(Arc::new(auth), fast(), limiter.clone()).unwrap()
    };
    let (a, b) = (make(), make());
    a.devices().list(&DeviceFilter::default()).await.unwrap();
    b.devices().list(&DeviceFilter::default()).await.unwrap();
    assert!(limiter.available() < 8.5, "two requests used two tokens of one bucket: {}", limiter.available());
    assert_eq!(b.quota(), Quota { limit: Some(3600), remaining: Some(3550) });
    assert_eq!(a.quota(), b.quota(), "the quota one client saw is the key's");
    assert!(Arc::ptr_eq(a.limiter(), b.limiter()));
}
