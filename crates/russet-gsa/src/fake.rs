// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `GsaFake` (feature `fake`; W22 acceptance): a local GrandSlam + developerservices2 server for the mocked suites.
//! It runs the server side of SRP-6a with a synthetic account, encrypts `spd` and app tokens like Apple does, plays
//! both two-factor flows, provisions anisette for `FakeAdi`, and answers `listTeams` and a few `v1` reads. All of
//! its data is synthetic.
//!
//! It enforces what GrandSlam enforces since 2026-10-12 for Android ADI anisette (the kind every Russet fake
//! stands for): `init` with a macOS `X-Mme-Client-Info`, or with a `cpd` client info that differs from the header,
//! answers `ec -80045`; a non-akd `User-Agent` gets a bare HTTP 503.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use russet_anisette::fake::FakeAdi;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::crypto::{app_token_checksum, hmac_sha256, seal};
use crate::session::SecondFactor;
use crate::srp::{Group, server};

/// The synthetic account.
#[derive(Debug, Clone)]
pub struct GsaFakeConfig {
    pub apple_id: String,
    pub password: String,
    /// `s2k` or `s2k_fo`.
    pub protocol: &'static str,
    /// `None`: no second factor.
    pub two_factor: Option<SecondFactor>,
    pub code: String,
    pub dsid: i64,
    pub adsid: String,
    pub team_id: String,
    pub team_name: String,
    pub paid: bool,
    /// Lifetime of the Xcode tokens it issues, in seconds.
    pub token_seconds: i64,
    /// How many `init` requests to answer with a bare HTTP 503 first (GrandSlam's answer to back-to-back logins).
    pub busy_inits: u32,
}

impl Default for GsaFakeConfig {
    fn default() -> Self {
        Self {
            apple_id: "canary.user@example.invalid".into(),
            password: "canary-password-0451".into(),
            protocol: "s2k",
            two_factor: Some(SecondFactor::TrustedDevice),
            code: "246810".into(),
            dsid: 7_000_000_042,
            adsid: "000777-08-canary-adsid".into(),
            team_id: "FAKETEAM42".into(),
            team_name: "Fake Personal Team".into(),
            paid: true,
            token_seconds: 3600,
            busy_inits: 0,
        }
    }
}

/// What the fake saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeStats {
    pub inits: u32,
    pub completes: u32,
    pub pushes: u32,
    pub sms_sent: Vec<u32>,
    pub codes_checked: u32,
    pub tokens_issued: u32,
    pub provisionings: u32,
    pub team_lists: u32,
    pub v1_requests: u32,
    /// `init` requests refused for their client identity (`-80045`).
    pub identity_rejections: u32,
    /// `gsService` requests answered with a bare 503.
    pub busy_answers: u32,
}

struct Handshake {
    hs: server::Handshake,
    a_pub: Vec<u8>,
    user: String,
}

struct State {
    cfg: GsaFakeConfig,
    group: Group,
    record: server::Record,
    handshakes: Vec<(String, Handshake)>,
    trusted: bool,
    sk: Vec<u8>,
    idms: String,
    tokens: Vec<String>,
    stats: FakeStats,
}

/// The fake server.
pub struct GsaFake {
    server: MockServer,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for GsaFake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GsaFake").field("uri", &self.server.uri()).finish_non_exhaustive()
    }
}

fn plist_bytes(v: plist::Value) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = plist::to_writer_xml(&mut out, &v);
    out
}

fn dict(entries: Vec<(&str, plist::Value)>) -> plist::Value {
    plist::Value::Dictionary(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn ok_status() -> plist::Value {
    dict(vec![("hsc", 200.into()), ("ec", 0.into()), ("em", "".into())])
}

fn plist_response(v: plist::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).insert_header("content-type", "text/x-xml-plist").set_body_bytes(plist_bytes(v))
}

fn gs_response(resp: Vec<(&str, plist::Value)>) -> ResponseTemplate {
    plist_response(dict(vec![("Response", dict(resp))]))
}

fn request_dict(req: &Request) -> plist::Dictionary {
    plist::from_bytes::<plist::Value>(&req.body)
        .ok()
        .and_then(|v| v.as_dictionary()?.get("Request")?.as_dictionary().cloned())
        .unwrap_or_default()
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

impl State {
    fn identity_ok(&self, req: &Request) -> bool {
        let expected = b64(format!("{}:{}", self.cfg.adsid, self.idms).as_bytes());
        req.headers.get("X-Apple-Identity-Token").and_then(|v| v.to_str().ok()) == Some(expected.as_str())
    }

    fn token_ok(&self, req: &Request) -> bool {
        let t = req.headers.get("X-Apple-GS-Token").and_then(|v| v.to_str().ok()).unwrap_or_default();
        let id = req.headers.get("X-Apple-I-Identity-Id").and_then(|v| v.to_str().ok()).unwrap_or_default();
        id == self.cfg.adsid && self.tokens.iter().any(|x| x == t) && req.headers.contains_key("X-Apple-I-MD")
    }

    fn gs_service(&mut self, req: &Request) -> ResponseTemplate {
        let r = request_dict(req);
        match r.get("o").and_then(plist::Value::as_string).unwrap_or_default() {
            "init" => self.init(&r, req),
            "complete" => self.complete(&r),
            "apptokens" => self.apptokens(&r),
            _ => ResponseTemplate::new(400),
        }
    }

    fn init(&mut self, r: &plist::Dictionary, req: &Request) -> ResponseTemplate {
        self.stats.inits += 1;
        if self.stats.busy_answers < self.cfg.busy_inits {
            self.stats.busy_answers += 1;
            return ResponseTemplate::new(503).set_body_string("<html>Service Temporarily Unavailable</html>");
        }
        let header = |k: &str| req.headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
        if !header("user-agent").starts_with("akd/") {
            self.stats.busy_answers += 1;
            return ResponseTemplate::new(503).set_body_string("<html>Service Temporarily Unavailable</html>");
        }
        let client_info = header("x-mme-client-info");
        let cpd_client_info = r
            .get("cpd")
            .and_then(plist::Value::as_dictionary)
            .and_then(|c| c.get("X-MMe-Client-Info"))
            .and_then(plist::Value::as_string)
            .unwrap_or_default();
        let mac = russet_anisette::platform_of(&client_info) == "macos";
        if mac || client_info.is_empty() || cpd_client_info != client_info {
            self.stats.identity_rejections += 1;
            return gs_response(vec![(
                "Status",
                dict(vec![
                    ("hsc", 500.into()),
                    ("ec", (-80045).into()),
                    ("em", "This action cannot be completed at this time.".into()),
                ]),
            )]);
        }
        let user = r.get("u").and_then(plist::Value::as_string).unwrap_or_default().to_owned();
        let a_pub = r.get("A2k").and_then(plist::Value::as_data).map(<[u8]>::to_vec).unwrap_or_default();
        let has_cpd =
            r.get("cpd").and_then(plist::Value::as_dictionary).is_some_and(|c| c.contains_key("X-Apple-I-MD"));
        if !has_cpd {
            return gs_response(vec![(
                "Status",
                dict(vec![("hsc", 400.into()), ("ec", (-36607).into()), ("em", "no anisette".into())]),
            )]);
        }
        let n = self.stats.inits;
        let b: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(31).wrapping_add(n as u8)).collect();
        let hs = server::start(&self.group, &self.record, &b);
        let c = format!("fake-c-{n}");
        let b_pub = self.group.pad(&hs.b_pub);
        self.handshakes.push((c.clone(), Handshake { hs, a_pub, user }));
        gs_response(vec![
            ("Status", ok_status()),
            ("sp", self.cfg.protocol.into()),
            ("i", i64::from(self.record.iterations).into()),
            ("s", plist::Value::Data(self.record.salt.clone())),
            ("B", plist::Value::Data(b_pub)),
            ("c", c.into()),
        ])
    }

    fn complete(&mut self, r: &plist::Dictionary) -> ResponseTemplate {
        self.stats.completes += 1;
        let c = r.get("c").and_then(plist::Value::as_string).unwrap_or_default();
        let m1 = r.get("M1").and_then(plist::Value::as_data).map(<[u8]>::to_vec).unwrap_or_default();
        let wrong = || {
            gs_response(vec![(
                "Status",
                dict(vec![
                    ("hsc", 401.into()),
                    ("ec", (-22406).into()),
                    ("em", "Your Apple ID or password was entered incorrectly.".into()),
                ]),
            )])
        };
        let Some(pos) = self.handshakes.iter().position(|(k, _)| k == c) else { return wrong() };
        let (_, h) = self.handshakes.remove(pos);
        if h.user != self.cfg.apple_id.to_lowercase() {
            return wrong();
        }
        let Some((expected, m2, key)) = server::finish(&self.group, &self.record, &h.hs, h.user.as_bytes(), &h.a_pub)
        else {
            return wrong();
        };
        if expected != m1 {
            return wrong();
        }
        let spd = dict(vec![
            ("adsid", self.cfg.adsid.clone().into()),
            ("GsIdmsToken", self.idms.clone().into()),
            ("sk", plist::Value::Data(self.sk.clone())),
            ("c", plist::Value::Data(b"fake-session-cookie".to_vec())),
            ("DsPrsId", self.cfg.dsid.into()),
            ("acname", self.cfg.apple_id.clone().into()),
        ]);
        let spd = seal::encrypt_spd(&key, &plist_bytes(spd));
        let mut status = vec![("hsc", 200.into()), ("ec", 0.into()), ("em", "".into())];
        if let (Some(kind), false) = (self.cfg.two_factor, self.trusted) {
            status[0] = ("hsc", 409.into());
            let au = match kind {
                SecondFactor::TrustedDevice => "trustedDeviceSecondaryAuth",
                SecondFactor::Sms => "secondaryAuth",
            };
            status.push(("au", au.into()));
        }
        gs_response(vec![
            ("Status", dict(status)),
            ("M2", plist::Value::Data(m2)),
            ("spd", plist::Value::Data(spd)),
            ("np", plist::Value::Data(hmac_sha256(&key, &[b"np"]))),
        ])
    }

    fn apptokens(&mut self, r: &plist::Dictionary) -> ResponseTemplate {
        let app = "com.apple.gs.xcode.auth";
        let checksum = r.get("checksum").and_then(plist::Value::as_data).unwrap_or_default();
        let t = r.get("t").and_then(plist::Value::as_string).unwrap_or_default();
        let u = r.get("u").and_then(plist::Value::as_string).unwrap_or_default();
        if checksum != app_token_checksum(&self.sk, &self.cfg.adsid, &[app]) || t != self.idms || u != self.cfg.adsid {
            return gs_response(vec![(
                "Status",
                dict(vec![("hsc", 401.into()), ("ec", (-22406).into()), ("em", "bad token".into())]),
            )]);
        }
        self.stats.tokens_issued += 1;
        let token = format!("fake-xcode-token-{}", self.stats.tokens_issued);
        self.tokens.push(token.clone());
        let expiry = (time::OffsetDateTime::now_utc().unix_timestamp() + self.cfg.token_seconds) * 1000;
        let plain = dict(vec![
            ("status-code", 200.into()),
            (
                "t",
                dict(vec![(
                    app,
                    dict(vec![
                        ("token", token.into()),
                        ("duration", self.cfg.token_seconds.into()),
                        ("expiry", expiry.into()),
                    ]),
                )]),
            ),
        ]);
        let et = seal::encrypt_gcm(&self.sk, &[7u8; 16], &plist_bytes(plain));
        gs_response(vec![("Status", ok_status()), ("et", plist::Value::Data(et))])
    }
}

struct Handler<F>(Arc<Mutex<State>>, F);

impl<F> Respond for Handler<F>
where
    F: Fn(&mut State, &Request) -> ResponseTemplate + Send + Sync + 'static,
{
    fn respond(&self, req: &Request) -> ResponseTemplate {
        match self.0.lock() {
            Ok(mut s) => (self.1)(&mut s, req),
            Err(_) => ResponseTemplate::new(500),
        }
    }
}

impl GsaFake {
    /// Starts a fake for `cfg`.
    pub async fn start(cfg: GsaFakeConfig) -> Self {
        let group = Group::gsa();
        let record = server::Record::new(&group, &cfg.password, b"russet-fake-salt", 1000, cfg.protocol);
        let state = Arc::new(Mutex::new(State {
            cfg,
            group,
            record,
            handshakes: Vec::new(),
            trusted: false,
            sk: vec![0x5a; 32],
            idms: "fake-idms-token".into(),
            tokens: Vec::new(),
            stats: FakeStats::default(),
        }));
        let server = MockServer::start().await;
        let s = |f: fn(&mut State, &Request) -> ResponseTemplate| Handler(state.clone(), f);

        Mock::given(method("GET"))
            .and(path("/grandslam/GsService2/lookup"))
            .respond_with(s(|_, _| {
                // Absolute production URLs: the client must rebase them onto the configured endpoint.
                let u = |p: &str| plist::Value::String(format!("https://gsa.apple.com/{p}"));
                plist_response(dict(vec![(
                    "urls",
                    dict(vec![
                        ("gsService", u("grandslam/GsService2")),
                        ("secondaryAuth", u("auth")),
                        ("trustedDeviceSecondaryAuth", u("auth/verify/trusteddevice")),
                        ("validateCode", u("grandslam/GsService2/validate")),
                        ("midStartProvisioning", u("grandslam/MidService/startMachineProvisioning")),
                        ("midFinishProvisioning", u("grandslam/MidService/finishMachineProvisioning")),
                    ]),
                )]))
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/grandslam/GsService2"))
            .respond_with(s(State::gs_service))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/auth/verify/trusteddevice"))
            .respond_with(s(|st, req| {
                if !st.identity_ok(req) {
                    return ResponseTemplate::new(401);
                }
                st.stats.pushes += 1;
                ResponseTemplate::new(200)
            }))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/grandslam/GsService2/validate"))
            .respond_with(s(|st, req| {
                if !st.identity_ok(req) {
                    return ResponseTemplate::new(401);
                }
                st.stats.codes_checked += 1;
                let code = req.headers.get("security-code").and_then(|v| v.to_str().ok()).unwrap_or_default();
                if code == st.cfg.code {
                    st.trusted = true;
                    plist_response(dict(vec![("ec", 0.into()), ("em", "".into())]))
                } else {
                    plist_response(dict(vec![("ec", (-21669).into()), ("em", "Incorrect verification code.".into())]))
                }
            }))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/auth"))
            .respond_with(s(|st, req| {
                if !st.identity_ok(req) {
                    return ResponseTemplate::new(401);
                }
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "trustedPhoneNumbers": [{"id": 1, "obfuscatedNumber": "(•••) •••-••12", "pushMode": "sms"}]
                }))
            }))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/auth/verify/phone"))
            .respond_with(s(|st, req| {
                let v: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
                match v["phoneNumber"]["id"].as_u64() {
                    Some(1) if st.identity_ok(req) => {
                        st.stats.sms_sent.push(1);
                        ResponseTemplate::new(200).set_body_json(serde_json::json!({}))
                    }
                    _ => ResponseTemplate::new(400),
                }
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/verify/phone/securitycode"))
            .respond_with(s(|st, req| {
                if !st.identity_ok(req) {
                    return ResponseTemplate::new(401);
                }
                st.stats.codes_checked += 1;
                let v: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
                if v["securityCode"]["code"].as_str() == Some(st.cfg.code.as_str()) && v["phoneNumber"]["id"] == 1 {
                    st.trusted = true;
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({}))
                } else {
                    ResponseTemplate::new(400)
                        .set_body_json(serde_json::json!({"service_errors": [{"code": "-21669", "message": "Incorrect verification code."}]}))
                }
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/grandslam/MidService/startMachineProvisioning"))
            .respond_with(s(|_, _| gs_response(vec![("Status", ok_status()), ("spim", b64(b"fake-spim").into())])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/grandslam/MidService/finishMachineProvisioning"))
            .respond_with(s(|st, req| {
                let r = request_dict(req);
                let cpim = r
                    .get("cpim")
                    .and_then(plist::Value::as_string)
                    .and_then(|c| base64::engine::general_purpose::STANDARD.decode(c).ok())
                    .unwrap_or_default();
                st.stats.provisionings += 1;
                gs_response(vec![
                    ("Status", ok_status()),
                    ("ptm", b64(&FakeAdi::ptm_for(&cpim)).into()),
                    ("tk", b64(&FakeAdi::tk_for(&cpim)).into()),
                    ("X-Apple-I-MD-RINFO", "17106176".into()),
                ])
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/services/QH65B2/listTeams.action"))
            .respond_with(s(|st, req| {
                if !st.token_ok(req) {
                    return plist_response(dict(vec![
                        ("resultCode", 1100.into()),
                        ("resultString", "Your session has expired.".into()),
                    ]));
                }
                st.stats.team_lists += 1;
                let membership =
                    if st.cfg.paid { "Apple Developer Program" } else { "Xcode Free Provisioning Program" };
                plist_response(dict(vec![
                    ("resultCode", 0.into()),
                    (
                        "teams",
                        plist::Value::Array(vec![dict(vec![
                            ("teamId", st.cfg.team_id.clone().into()),
                            ("name", st.cfg.team_name.clone().into()),
                            ("type", "Individual".into()),
                            ("status", "active".into()),
                            (
                                "memberships",
                                plist::Value::Array(vec![dict(vec![
                                    ("name", membership.into()),
                                    ("platform", "ios".into()),
                                ])]),
                            ),
                        ])]),
                    ),
                ]))
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"^/services/v1/.*"))
            .respond_with(s(|st, req| {
                let over = req.headers.get("X-HTTP-Method-Override").and_then(|v| v.to_str().ok()).unwrap_or_default();
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
                let team_ok = body["urlEncodedQueryParams"].as_str().is_some_and(|q| q.contains(&format!("teamId={}", st.cfg.team_id)))
                    || body["data"]["attributes"]["teamId"] == st.cfg.team_id.as_str();
                if !st.token_ok(req) || !team_ok {
                    return ResponseTemplate::new(401).set_body_json(serde_json::json!({"errors": [{"status": "401", "code": "NOT_AUTHORIZED"}]}));
                }
                st.stats.v1_requests += 1;
                if over == "GET" {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": [], "links": {}, "meta": {"paging": {"total": 0}}}))
                } else {
                    ResponseTemplate::new(403).set_body_json(serde_json::json!({"errors": [{"status": "403", "code": "FORBIDDEN_ERROR", "detail": "the fake only reads"}]}))
                }
            }))
            .mount(&server)
            .await;
        Self { server, state }
    }

    /// The GrandSlam base URL (`Endpoints.gsa`).
    pub fn gsa_url(&self) -> Url {
        Url::parse(&format!("{}/", self.server.uri())).expect("the mock server's URI is a URL")
    }

    /// The developerservices2 service root.
    pub fn ds2_url(&self) -> Url {
        self.gsa_url().join("services/").expect("a relative path joins")
    }

    pub fn stats(&self) -> FakeStats {
        self.state.lock().map(|s| s.stats.clone()).unwrap_or_default()
    }

    /// Forgets that this machine passed two-factor authentication.
    pub fn untrust(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.trusted = false;
        }
    }

    /// Every request the fake received (bodies and headers), for checks that no secret travels in clear.
    pub async fn received(&self) -> Vec<Request> {
        self.server.received_requests().await.unwrap_or_default()
    }
}
