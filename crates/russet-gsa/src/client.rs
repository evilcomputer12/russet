// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The GrandSlam client (SIGN §7.2–§7.3): SRP login, the two-factor flows and the Xcode app token.
//!
//! Requests carry the anisette headers in `cpd` (SRP) or as HTTP headers (two-factor), the profile's
//! `X-Mme-Client-Info`, and nothing is logged beyond the step names: never the Apple ID, password, codes, tokens,
//! `adsid` or anisette values.

use std::sync::Arc;

use russet_anisette::provisioning::{LookupUrls, lookup, rebase};
use russet_anisette::{AnisetteHeaders, AnisetteProvider, ClientIdentity};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::net::Endpoints;
use time::OffsetDateTime;
use url::Url;
use zeroize::Zeroizing;

use crate::crypto::{app_token_checksum, decrypt_gcm, decrypt_spd};
use crate::profile::ClientProfile;
use crate::session::{
    Cookie, GsSession, LoginState, PendingTwoFactor, SecondFactor, TrustedNumber, TwoFactorVia, XcodeToken,
};
use crate::srp::{Client as SrpClient, Group, password_key};

/// A GrandSlam client for one anisette provider.
pub struct GsaClient {
    http: reqwest::Client,
    base: Url,
    anisette: Arc<dyn AnisetteProvider>,
    profile: ClientProfile,
    urls: tokio::sync::Mutex<Option<LookupUrls>>,
}

impl std::fmt::Debug for GsaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GsaClient").field("base", &self.base.as_str()).finish_non_exhaustive()
    }
}

/// `pending` and `?` for plist lookups.
fn d_get<'a>(d: &'a plist::Dictionary, k: &str) -> Option<&'a plist::Value> {
    d.get(k)
}

fn as_i64(v: &plist::Value) -> Option<i64> {
    match v {
        plist::Value::Integer(i) => i.as_signed(),
        plist::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn data(v: Option<&plist::Value>) -> Option<Vec<u8>> {
    match v? {
        plist::Value::Data(d) => Some(d.clone()),
        plist::Value::String(s) => {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()
        }
        _ => None,
    }
}

pub(crate) fn protocol(msg: impl Into<String>) -> Error {
    Error::new(codes::gsa::PROTOCOL, msg).with_hint(
        "Apple changed something Russet relies on, or a proxy altered the answer; try again, and update Russet if it persists.",
    )
}

/// Masks anything that looks like an e-mail address in Apple's own messages (they are shown to the user).
pub(crate) fn scrub(text: &str) -> String {
    text.split(' ')
        .map(|w| if w.contains('@') && w.contains('.') { "<apple id>" } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

fn bad_credentials() -> Error {
    Error::new(codes::gsa::BAD_CREDENTIALS, "Apple did not accept this Apple ID and password")
        .with_hint("Check the Apple ID and password (an app-specific password does not work here) and try again.")
}

fn identity_rejected(detail: &str) -> Error {
    Error::new(codes::gsa::IDENTITY_REJECTED, format!("Apple refused this machine's sign-in identity: {detail}"))
        .with_hint("Apple rejected the client or anisette identity (it changes them from time to time). Update Russet (it refreshes gsa-profile.json), or reset anisette with `russet account anisette reset`, then sign in again.")
        .retryable(true)
}

/// The error of a GrandSlam `Status` (`ec != 0`), if any.
fn status_error(status: &plist::Dictionary) -> Option<Error> {
    let ec = status.get("ec").and_then(as_i64).unwrap_or(0);
    if ec == 0 {
        return None;
    }
    let em = scrub(status.get("em").and_then(plist::Value::as_string).unwrap_or_default());
    Some(match ec {
        -22406 => bad_credentials(),
        -20209 | -20283 | -22421 | -20751 => {
            Error::new(codes::gsa::LOCKED, format!("Apple reports this Apple ID is locked ({ec}): {em}"))
                .with_hint("Unlock it at https://iforgot.apple.com, then sign in again.")
        }
        -36607 | -45061 | -45054 => identity_rejected(&format!("{ec}: {em}")),
        _ => protocol(format!("Apple refused the sign-in ({ec}): {em}")),
    })
}

impl GsaClient {
    /// A client over `http` (see [`russet_anisette::apple_tls::gsa_client`]) for GrandSlam at `base`.
    pub fn new(http: reqwest::Client, base: Url, anisette: Arc<dyn AnisetteProvider>, profile: ClientProfile) -> Self {
        Self { http, base, anisette, profile, urls: tokio::sync::Mutex::new(None) }
    }

    pub fn profile(&self) -> &ClientProfile {
        &self.profile
    }

    pub fn anisette(&self) -> &Arc<dyn AnisetteProvider> {
        &self.anisette
    }

    /// `X-Mme-Client-Info`: the profile's override, else the anisette provider's.
    async fn client_info(&self) -> Result<String> {
        match &self.profile.anisette_client_info_override {
            Some(c) => Ok(c.clone()),
            None => self.anisette.client_info().await,
        }
    }

    async fn identity(&self) -> Result<ClientIdentity> {
        Ok(ClientIdentity { client_info: self.client_info().await?, user_agent: self.profile.user_agent.clone() })
    }

    /// The service URLs (`lookup`, once).
    pub async fn urls(&self) -> Result<LookupUrls> {
        let mut g = self.urls.lock().await;
        if let Some(u) = g.clone() {
            return Ok(u);
        }
        let u = lookup(&self.http, &self.base, &self.identity().await?).await?;
        *g = Some(u.clone());
        Ok(u)
    }

    fn cpd(h: &AnisetteHeaders) -> plist::Value {
        let mut d = plist::Dictionary::new();
        d.insert("bootstrap".into(), true.into());
        d.insert("icscrec".into(), true.into());
        d.insert("pbe".into(), false.into());
        d.insert("prkgen".into(), true.into());
        d.insert("svct".into(), "iCloud".into());
        d.insert("loc".into(), h.locale.clone().into());
        for (k, v) in h.pairs() {
            d.insert(k.into(), v.into());
        }
        plist::Value::Dictionary(d)
    }

    /// POSTs `{Header:{Version:"1.0.1"}, Request}` to `gsService` and returns `Response` (status not checked).
    async fn gs_post(&self, request: plist::Dictionary) -> Result<plist::Dictionary> {
        let urls = self.urls().await?;
        Endpoints::check(&urls.gs_service)?;
        let mut header = plist::Dictionary::new();
        header.insert("Version".into(), "1.0.1".into());
        let mut root = plist::Dictionary::new();
        root.insert("Header".into(), plist::Value::Dictionary(header));
        root.insert("Request".into(), plist::Value::Dictionary(request));
        let mut body = Vec::new();
        plist::to_writer_xml(&mut body, &plist::Value::Dictionary(root))
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode a GrandSlam request: {e}")))?;
        let resp = self
            .http
            .post(urls.gs_service.clone())
            .header(reqwest::header::CONTENT_TYPE, "text/x-xml-plist")
            .header(reqwest::header::ACCEPT, "*/*")
            .header(reqwest::header::USER_AGENT, &self.profile.user_agent)
            .header("X-Mme-Client-Info", self.client_info().await?)
            .header(reqwest::header::CONNECTION, "close")
            .body(body)
            .send()
            .await
            .map_err(|e| unavailable(format!("GrandSlam is unreachable: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| unavailable(e.without_url().to_string()))?;
        if status == 429 || status == 503 {
            return Err(identity_rejected(&format!("HTTP {status}")));
        }
        if status >= 500 {
            return Err(unavailable(format!("GrandSlam answered HTTP {status}")));
        }
        let v: plist::Value = plist::from_bytes(&bytes)
            .map_err(|_| protocol(format!("GrandSlam's answer is not a plist (HTTP {status})")))?;
        v.as_dictionary()
            .and_then(|d| d.get("Response"))
            .and_then(plist::Value::as_dictionary)
            .cloned()
            .ok_or_else(|| protocol("GrandSlam's answer has no Response"))
    }

    /// SRP login (`init` + `complete`). Apple ID matching is case-insensitive; the lowercase form is sent.
    pub async fn login(&self, apple_id: &str, password: &[u8]) -> Result<LoginState> {
        let user = apple_id.trim().to_lowercase();
        let anisette = self.anisette.headers().await?;
        let srp = SrpClient::new(Group::gsa());

        let mut init = plist::Dictionary::new();
        init.insert("A2k".into(), plist::Value::Data(srp.a_pub()));
        init.insert("cpd".into(), Self::cpd(&anisette));
        init.insert("o".into(), "init".into());
        init.insert("ps".into(), plist::Value::Array(vec!["s2k".into(), "s2k_fo".into()]));
        init.insert("u".into(), user.clone().into());
        let r = self.gs_post(init).await?;
        if let Some(e) = r.get("Status").and_then(plist::Value::as_dictionary).and_then(status_error) {
            return Err(e);
        }
        let sp = r.get("sp").and_then(plist::Value::as_string).unwrap_or("s2k").to_owned();
        if sp != "s2k" && sp != "s2k_fo" {
            return Err(protocol(format!("GrandSlam chose the unsupported protocol {sp}")));
        }
        let iterations = r
            .get("i")
            .and_then(as_i64)
            .and_then(|i| u32::try_from(i).ok())
            .ok_or_else(|| protocol("init has no iteration count"))?;
        let salt = data(r.get("s")).ok_or_else(|| protocol("init has no salt"))?;
        let b_pub = data(r.get("B")).ok_or_else(|| protocol("init has no B"))?;
        let c = r.get("c").cloned().ok_or_else(|| protocol("init has no c"))?;
        let key = password_key(password, &salt, iterations, &sp);
        let proof = srp
            .process(user.as_bytes(), &salt, &key, &b_pub)
            .ok_or_else(|| protocol("GrandSlam sent an unsafe SRP value"))?;
        tracing::debug!(protocol = %sp, "gsa: srp init done");

        let anisette = self.anisette.headers().await?;
        let mut complete = plist::Dictionary::new();
        complete.insert("M1".into(), plist::Value::Data(proof.m1.clone()));
        complete.insert("c".into(), c);
        complete.insert("cpd".into(), Self::cpd(&anisette));
        complete.insert("o".into(), "complete".into());
        complete.insert("u".into(), user.into());
        let r = self.gs_post(complete).await?;
        let status = r.get("Status").and_then(plist::Value::as_dictionary).cloned().unwrap_or_default();
        if let Some(e) = status_error(&status) {
            return Err(e);
        }
        let m2 = data(r.get("M2")).ok_or_else(|| protocol("complete has no M2"))?;
        if m2 != proof.m2 {
            return Err(protocol("GrandSlam's SRP proof (M2) does not match; the connection is not trustworthy"));
        }
        let spd = data(r.get("spd")).ok_or_else(|| protocol("complete has no spd"))?;
        let plain = decrypt_spd(&proof.key, &spd).ok_or_else(|| protocol("cannot decrypt spd"))?;
        let session = parse_spd(&plain)?;
        tracing::debug!("gsa: srp complete");
        let au = status.get("au").and_then(plist::Value::as_string).unwrap_or_default();
        let hsc = status.get("hsc").and_then(as_i64).unwrap_or(200);
        Ok(match au {
            "trustedDeviceSecondaryAuth" => {
                LoginState::NeedsTwoFactor(PendingTwoFactor { session, kind: SecondFactor::TrustedDevice })
            }
            "secondaryAuth" => LoginState::NeedsTwoFactor(PendingTwoFactor { session, kind: SecondFactor::Sms }),
            "" if hsc == 409 => {
                LoginState::NeedsTwoFactor(PendingTwoFactor { session, kind: SecondFactor::TrustedDevice })
            }
            "" => LoginState::Authenticated(session),
            other => return Err(protocol(format!("Apple asks for an unsupported sign-in step ({other})"))),
        })
    }

    /// The headers of the two-factor requests.
    async fn second_factor_headers(&self, s: &GsSession, json: bool) -> Result<reqwest::header::HeaderMap> {
        let mut pairs: Vec<(String, String)> = vec![
            ("X-Apple-Identity-Token".into(), s.identity_token().to_string()),
            ("X-Apple-App-Info".into(), self.profile.app_info.clone()),
            ("X-Xcode-Version".into(), self.profile.xcode_version.clone()),
            ("X-Mme-Client-Info".into(), self.client_info().await?),
            ("User-Agent".into(), "Xcode".into()),
            ("Accept-Language".into(), "en-us".into()),
        ];
        if json {
            pairs.push(("Content-Type".into(), "application/json".into()));
            pairs.push(("Accept".into(), "application/json, text/javascript".into()));
        } else {
            pairs.push(("Content-Type".into(), "text/x-xml-plist".into()));
            pairs.push(("Accept".into(), "text/x-xml-plist".into()));
        }
        pairs.extend(self.anisette.headers().await?.pairs().into_iter().map(|(k, v)| (k.to_owned(), v)));
        header_map(&pairs)
    }

    /// The trusted phone numbers (`GET <secondaryAuth>`), in Apple's obfuscated form.
    pub async fn trusted_numbers(&self, p: &PendingTwoFactor) -> Result<Vec<TrustedNumber>> {
        let urls = self.urls().await?;
        Endpoints::check(&urls.secondary_auth)?;
        let resp = self
            .http
            .get(urls.secondary_auth.clone())
            .headers(self.second_factor_headers(&p.session, true).await?)
            .send()
            .await
            .map_err(|e| unavailable(format!("cannot list the trusted phone numbers: {}", e.without_url())))?;
        let text = resp.text().await.map_err(|e| unavailable(e.without_url().to_string()))?;
        Ok(parse_trusted_numbers(&text))
    }

    /// Asks Apple to send a code: a push to the trusted devices, or an SMS.
    pub async fn request_code(&self, p: &PendingTwoFactor, via: TwoFactorVia) -> Result<()> {
        let urls = self.urls().await?;
        let resp = match via {
            TwoFactorVia::TrustedDevice => {
                Endpoints::check(&urls.trusted_device_secondary_auth)?;
                self.http
                    .get(urls.trusted_device_secondary_auth.clone())
                    .headers(self.second_factor_headers(&p.session, false).await?)
                    .send()
                    .await
            }
            TwoFactorVia::Sms { phone_id } => {
                let url = self.phone_url("auth/verify/phone")?;
                Endpoints::check(&url)?;
                self.http
                    .put(url)
                    .headers(self.second_factor_headers(&p.session, true).await?)
                    .body(serde_json::json!({"phoneNumber": {"id": phone_id}, "mode": "sms"}).to_string())
                    .send()
                    .await
            }
        }
        .map_err(|e| unavailable(format!("cannot request a two-factor code: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        match status {
            200..=299 => Ok(()),
            423 | 429 => Err(rate_limited()),
            _ => Err(protocol(format!("Apple answered HTTP {status} to the code request"))),
        }
    }

    fn phone_url(&self, path: &str) -> Result<Url> {
        rebase(&self.base, &format!("/{path}")).ok_or_else(|| Error::invalid_argument("invalid GrandSlam endpoint"))
    }

    /// Checks a code. On success Apple trusts this machine's anisette identity; log in again to get a session.
    pub async fn submit_code(&self, p: &PendingTwoFactor, via: TwoFactorVia, code: &str) -> Result<()> {
        let code = code.trim();
        if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad_code());
        }
        let urls = self.urls().await?;
        match via {
            TwoFactorVia::TrustedDevice => {
                Endpoints::check(&urls.validate_code)?;
                let mut headers = self.second_factor_headers(&p.session, false).await?;
                headers.insert("security-code", reqwest::header::HeaderValue::from_str(code).map_err(|_| bad_code())?);
                let resp = self
                    .http
                    .get(urls.validate_code.clone())
                    .headers(headers)
                    .send()
                    .await
                    .map_err(|e| unavailable(format!("cannot check the code: {}", e.without_url())))?;
                let status = resp.status().as_u16();
                if status == 429 || status == 423 {
                    return Err(rate_limited());
                }
                let bytes = resp.bytes().await.map_err(|e| unavailable(e.without_url().to_string()))?;
                let v: plist::Value = plist::from_bytes(&bytes)
                    .map_err(|_| protocol(format!("the code check answered HTTP {status}")))?;
                let d = v.as_dictionary().cloned().unwrap_or_default();
                let st = d.get("Status").and_then(plist::Value::as_dictionary).cloned().unwrap_or_else(|| d.clone());
                let ec = st.get("ec").and_then(as_i64).unwrap_or(if status < 300 { 0 } else { -1 });
                match ec {
                    0 => Ok(()),
                    -21669 => Err(bad_code()),
                    -20209 | -20283 => Err(rate_limited()),
                    _ => Err(bad_code()),
                }
            }
            TwoFactorVia::Sms { phone_id } => {
                let url = self.phone_url("auth/verify/phone/securitycode")?;
                Endpoints::check(&url)?;
                let resp = self
                    .http
                    .post(url)
                    .headers(self.second_factor_headers(&p.session, true).await?)
                    .body(
                        serde_json::json!({"securityCode": {"code": code}, "phoneNumber": {"id": phone_id}, "mode": "sms"})
                            .to_string(),
                    )
                    .send()
                    .await
                    .map_err(|e| unavailable(format!("cannot check the code: {}", e.without_url())))?;
                match resp.status().as_u16() {
                    200..=299 => Ok(()),
                    423 | 429 => Err(rate_limited()),
                    _ => Err(bad_code()),
                }
            }
        }
    }

    /// The Xcode app token (`apptokens`).
    pub async fn xcode_token(&self, s: &GsSession) -> Result<XcodeToken> {
        let app = self.profile.app_info.clone();
        let anisette = self.anisette.headers().await?;
        let mut req = plist::Dictionary::new();
        req.insert("app".into(), plist::Value::Array(vec![app.clone().into()]));
        req.insert("c".into(), s.c.to_plist());
        req.insert("checksum".into(), plist::Value::Data(app_token_checksum(&s.sk, &s.adsid, &[&app])));
        req.insert("cpd".into(), Self::cpd(&anisette));
        req.insert("o".into(), "apptokens".into());
        req.insert("t".into(), s.idms_token.as_str().into());
        req.insert("u".into(), s.adsid.clone().into());
        let r = self.gs_post(req).await?;
        if let Some(e) = r.get("Status").and_then(plist::Value::as_dictionary).and_then(status_error) {
            return Err(if e.code() == codes::gsa::BAD_CREDENTIALS { session_expired() } else { e });
        }
        let et = data(r.get("et")).ok_or_else(|| protocol("apptokens has no et"))?;
        let plain = decrypt_gcm(&s.sk, &et).ok_or_else(|| protocol("cannot decrypt the app token"))?;
        let v: plist::Value = plist::from_bytes(&plain).map_err(|_| protocol("the app token is not a plist"))?;
        let d = v.as_dictionary().cloned().unwrap_or_default();
        let code = d.get("status-code").and_then(as_i64).unwrap_or(200);
        if code != 200 {
            return Err(session_expired());
        }
        let t = d
            .get("t")
            .and_then(plist::Value::as_dictionary)
            .and_then(|t| t.get(&app))
            .and_then(plist::Value::as_dictionary)
            .ok_or_else(|| protocol("the app token answer has no Xcode token"))?;
        let token = t.get("token").and_then(plist::Value::as_string).ok_or_else(|| protocol("no token"))?;
        let now = OffsetDateTime::now_utc();
        let expires_at = match (t.get("expiry").and_then(as_i64), t.get("duration").and_then(as_i64)) {
            (Some(ms), _) if ms > 0 => OffsetDateTime::from_unix_timestamp(ms / 1000).unwrap_or(now),
            (_, Some(secs)) => now + time::Duration::seconds(secs),
            _ => now + time::Duration::hours(1),
        };
        tracing::debug!("gsa: xcode token issued");
        Ok(XcodeToken { adsid: s.adsid.clone(), token: Zeroizing::new(token.to_owned()), expires_at })
    }

    /// The headers developerservices2 needs with `token` (the Apple ID's session, the Xcode identity and fresh
    /// anisette).
    pub async fn ds2_headers(&self, token: &XcodeToken) -> Result<Vec<(String, String)>> {
        let mut pairs: Vec<(String, String)> = vec![
            ("X-Apple-I-Identity-Id".into(), token.adsid.clone()),
            ("X-Apple-GS-Token".into(), token.token.to_string()),
            ("X-Apple-App-Info".into(), self.profile.app_info.clone()),
            ("X-Xcode-Version".into(), self.profile.xcode_version.clone()),
            ("X-Mme-Client-Info".into(), self.client_info().await?),
        ];
        pairs.extend(self.anisette.headers().await?.pairs().into_iter().map(|(k, v)| (k.to_owned(), v)));
        Ok(pairs)
    }
}

pub(crate) fn header_map(pairs: &[(String, String)]) -> Result<reqwest::header::HeaderMap> {
    let mut m = reqwest::header::HeaderMap::new();
    for (k, v) in pairs {
        let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| Error::new(codes::core::INTERNAL, format!("invalid header name {k}")))?;
        let value = reqwest::header::HeaderValue::from_str(v)
            .map_err(|_| Error::new(codes::core::INTERNAL, format!("the header {k} has an invalid value")))?;
        m.insert(name, value);
    }
    Ok(m)
}

pub(crate) fn unavailable(msg: impl Into<String>) -> Error {
    Error::new(codes::account::APPLE_SERVICE_UNAVAILABLE, msg)
        .with_hint("Check the network; Apple's sign-in service may be briefly unavailable.")
        .retryable(true)
}

fn bad_code() -> Error {
    Error::new(codes::gsa::BAD_CODE, "Apple did not accept the verification code").with_hint(
        "Enter the 6-digit code shown on your trusted device or sent by text message; request a new one if it expired.",
    )
}

fn rate_limited() -> Error {
    Error::new(codes::gsa::CODE_RATE_LIMITED, "Apple refuses more verification codes for now")
        .with_hint("Wait a while (Apple limits code requests and attempts), then start the sign-in again.")
        .retryable(true)
}

pub(crate) fn session_expired() -> Error {
    Error::new(codes::gsa::SESSION_EXPIRED, "the Apple ID session has expired")
        .with_hint("Sign in again (the Apple ID page of the IDE, or `russet account apple-id login`).")
}

/// The session fields of a decrypted `spd`.
fn parse_spd(plain: &[u8]) -> Result<GsSession> {
    let v: plist::Value = plist::from_bytes(plain).map_err(|_| protocol("spd is not a plist"))?;
    let d = v.as_dictionary().ok_or_else(|| protocol("spd is not a dictionary"))?;
    let s = |k: &str| d_get(d, k).and_then(plist::Value::as_string).map(str::to_owned);
    let adsid = s("adsid").ok_or_else(|| protocol("spd has no adsid"))?;
    let dsid = match d_get(d, "DsPrsId") {
        Some(plist::Value::Integer(i)) => i.to_string(),
        Some(plist::Value::String(x)) => x.clone(),
        _ => adsid.clone(),
    };
    Ok(GsSession {
        dsid,
        idms_token: Zeroizing::new(s("GsIdmsToken").ok_or_else(|| protocol("spd has no GsIdmsToken"))?),
        sk: Zeroizing::new(data(d_get(d, "sk")).ok_or_else(|| protocol("spd has no sk"))?),
        c: d_get(d, "c").and_then(Cookie::from_plist).ok_or_else(|| protocol("spd has no c"))?,
        adsid,
    })
}

/// `trustedPhoneNumbers` from `GET /auth`: JSON, or the JSON boot arguments of Apple's HTML page.
pub(crate) fn parse_trusted_numbers(text: &str) -> Vec<TrustedNumber> {
    let json = serde_json::from_str::<serde_json::Value>(text).ok().or_else(|| {
        let start = text.find("boot_args")?;
        let open = start + text[start..].find('>')? + 1;
        let close = open + text[open..].find("</script>")?;
        serde_json::from_str(&text[open..close]).ok()
    });
    fn find(v: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
        match v {
            serde_json::Value::Object(m) => {
                m.get("trustedPhoneNumbers").and_then(serde_json::Value::as_array).or_else(|| m.values().find_map(find))
            }
            serde_json::Value::Array(a) => a.iter().find_map(find),
            _ => None,
        }
    }
    json.as_ref()
        .and_then(find)
        .map(|list| {
            list.iter()
                .filter_map(|n| {
                    let id = u32::try_from(n.get("id")?.as_u64()?).ok()?;
                    let obfuscated = n
                        .get("obfuscatedNumber")
                        .or_else(|| n.get("numberWithDialCode"))
                        .and_then(serde_json::Value::as_str)
                        .map(obfuscate)?;
                    Some(TrustedNumber { id, obfuscated })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Keeps Apple's obfuscation, and hides all but the last two digits of anything that still shows more.
fn obfuscate(number: &str) -> String {
    let digits = number.chars().filter(char::is_ascii_digit).count();
    if digits <= 2 {
        return number.to_owned();
    }
    let mut seen = 0;
    number
        .chars()
        .map(|c| {
            if c.is_ascii_digit() {
                seen += 1;
                if seen <= digits - 2 { '•' } else { c }
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_numbers_come_from_json_or_html_and_stay_obfuscated() {
        let json = r#"{"trustedPhoneNumbers":[{"id":1,"obfuscatedNumber":"(•••) •••-••12","pushMode":"sms"}]}"#;
        assert_eq!(parse_trusted_numbers(json), vec![TrustedNumber { id: 1, obfuscated: "(•••) •••-••12".into() }]);
        let html = r#"<html><script type="application/json" class="boot_args">{"direct":{"twoSV":{"phoneNumberVerification":{"trustedPhoneNumbers":[{"id":2,"numberWithDialCode":"+1 (555) 010-0199"}]}}}}</script></html>"#;
        assert_eq!(parse_trusted_numbers(html), vec![TrustedNumber { id: 2, obfuscated: "+• (•••) •••-••99".into() }]);
        assert!(parse_trusted_numbers("nothing").is_empty());
    }

    #[test]
    fn statuses_map_to_russet_codes() {
        let st = |ec: i64| {
            let mut d = plist::Dictionary::new();
            d.insert("ec".into(), plist::Value::Integer(ec.into()));
            d.insert("em".into(), "Your someone@example.com account".into());
            status_error(&d).map(|e| (e.code(), e.message().to_owned()))
        };
        assert_eq!(st(0), None);
        assert_eq!(st(-22406).map(|e| e.0), Some(codes::gsa::BAD_CREDENTIALS));
        assert_eq!(st(-20209).map(|e| e.0), Some(codes::gsa::LOCKED));
        assert_eq!(st(-36607).map(|e| e.0), Some(codes::gsa::IDENTITY_REJECTED));
        let other = st(-1).expect("an error");
        assert_eq!(other.0, codes::gsa::PROTOCOL);
        assert!(!other.1.contains("someone@example.com"), "{}", other.1);
    }
}
