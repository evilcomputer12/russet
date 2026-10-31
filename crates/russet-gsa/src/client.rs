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
    retry_delay: std::time::Duration,
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

/// How long the one retry of a login answered with a bare 503 waits (back-to-back SRP logins; 5–15 s works).
pub const BUSY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(8);

/// GrandSlam's front end answered a bare 503 (no plist).
fn busy(e: &Error) -> bool {
    e.details().and_then(|d| d.get("httpStatus")).and_then(serde_json::Value::as_u64) == Some(503)
}

/// The error of a GrandSlam `Status` (`ec != 0`) at `stage`, if any.
fn status_error(status: &plist::Dictionary, stage: &str) -> Option<Error> {
    let ec = status.get("ec").and_then(as_i64).unwrap_or(0);
    if ec == 0 {
        return None;
    }
    let em = scrub(status.get("em").and_then(plist::Value::as_string).unwrap_or_default());
    Some(match ec {
        // Since 2026-10-12 GrandSlam refuses a client info that does not match the platform of the anisette data
        // (-80045 at init), and Android-identifying clients at two-factor (-80046).
        -80045 | -80046 => Error::new(
            codes::gsa::IDENTITY_REJECTED,
            format!("Apple refused this client identity at {stage} ({ec}): {em}"),
        )
        .with_hint("GrandSlam rejects a client identity that does not match the anisette in use. Update Russet (it refreshes gsa-profile.json); a remote anisette server presents its own matching identity.")
        .with_details(serde_json::json!({"ec": ec, "stage": stage})),
        -22406 => bad_credentials(),
        -20209 | -20283 | -22421 | -20751 => {
            Error::new(codes::gsa::LOCKED, format!("Apple reports this Apple ID is locked ({ec}): {em}"))
                .with_hint("Unlock it at https://iforgot.apple.com, then sign in again.")
        }
        -36607 | -45061 | -45054 => identity_rejected(&format!("{ec} at {stage}: {em}")),
        _ => protocol(format!("Apple refused the sign-in at {stage} ({ec}): {em}"))
            .with_details(serde_json::json!({"ec": ec, "stage": stage})),
    })
}

impl GsaClient {
    /// A client over `http` (see [`russet_anisette::apple_tls::gsa_client`]) for GrandSlam at `base`.
    pub fn new(http: reqwest::Client, base: Url, anisette: Arc<dyn AnisetteProvider>, profile: ClientProfile) -> Self {
        Self { http, base, anisette, profile, urls: tokio::sync::Mutex::new(None), retry_delay: BUSY_RETRY_DELAY }
    }

    /// Waits `delay` before the one retry of a login GrandSlam answered with a bare 503 (tests use zero).
    pub fn with_retry_delay(mut self, delay: std::time::Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    pub fn profile(&self) -> &ClientProfile {
        &self.profile
    }

    pub fn anisette(&self) -> &Arc<dyn AnisetteProvider> {
        &self.anisette
    }

    /// The identity every GrandSlam request presents: the anisette provider's `X-Mme-Client-Info` and akd
    /// `User-Agent` (GrandSlam refuses a client info that does not match the platform the anisette data comes
    /// from, `-80045`), with the profile's client-info override.
    async fn identity(&self) -> Result<ClientIdentity> {
        let mut id = self.anisette.identity().await?;
        if let Some(c) = &self.profile.anisette_client_info_override {
            id.client_info = c.clone();
        }
        Ok(id)
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

    fn cpd(h: &AnisetteHeaders, id: &ClientIdentity) -> plist::Value {
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
        // Working clients repeat the client info inside cpd (xtool's DeviceInfo, anisette-v3 clients).
        d.insert("X-MMe-Client-Info".into(), id.client_info.clone().into());
        plist::Value::Dictionary(d)
    }

    /// POSTs `{Header:{Version:"1.0.1"}, Request}` to `gsService` for `stage` and returns `Response` (its
    /// `Status` is logged at debug level, not checked). Logs name headers and `cpd` keys, never their values.
    async fn gs_post(
        &self,
        stage: &'static str,
        request: plist::Dictionary,
        id: &ClientIdentity,
    ) -> Result<plist::Dictionary> {
        let urls = self.urls().await?;
        Endpoints::check(&urls.gs_service)?;
        let cpd_keys: Vec<String> = request
            .get("cpd")
            .and_then(plist::Value::as_dictionary)
            .map(|d| d.keys().cloned().collect())
            .unwrap_or_default();
        tracing::debug!(
            stage,
            platform = russet_anisette::platform_of(&id.client_info),
            headers = ?["Content-Type", "Accept", "User-Agent", "X-Mme-Client-Info", "Connection"],
            cpd = ?cpd_keys,
            "gsa: request"
        );
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
            .header(reqwest::header::USER_AGENT, &id.user_agent)
            .header("X-Mme-Client-Info", &id.client_info)
            .header(reqwest::header::CONNECTION, "close")
            .body(body)
            .send()
            .await
            .map_err(|e| unavailable(format!("GrandSlam is unreachable at {stage}: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| unavailable(e.without_url().to_string()))?;
        // An error status still carries a plist Status (ec/em) when GrandSlam itself refused; only a bare answer
        // (an HTML 503 from the front end) is judged by its HTTP status.
        let response = plist::from_bytes::<plist::Value>(&bytes)
            .ok()
            .and_then(|v| v.as_dictionary()?.get("Response")?.as_dictionary().cloned());
        let Some(response) = response else {
            tracing::debug!(stage, http = status, "gsa: response without a plist");
            return Err(match status {
                429 | 503 => identity_rejected(&format!("HTTP {status} at {stage}"))
                    .with_details(serde_json::json!({"httpStatus": status, "stage": stage})),
                s if s >= 500 => unavailable(format!("GrandSlam answered HTTP {s} at {stage}")),
                s => protocol(format!("GrandSlam's answer to {stage} is not a plist (HTTP {s})")),
            });
        };
        let st = response.get("Status").and_then(plist::Value::as_dictionary);
        let field = |k: &str| st.and_then(|s| s.get(k)).and_then(as_i64).unwrap_or(0);
        tracing::debug!(
            stage,
            http = status,
            hsc = field("hsc"),
            ec = field("ec"),
            em = %scrub(st.and_then(|s| s.get("em")).and_then(plist::Value::as_string).unwrap_or_default()),
            au = st.and_then(|s| s.get("au")).and_then(plist::Value::as_string).unwrap_or_default(),
            "gsa: response"
        );
        Ok(response)
    }

    /// SRP login (`init` + `complete`). Apple ID matching is case-insensitive; the lowercase form is sent. A bare
    /// HTTP 503 (GrandSlam's answer to back-to-back logins) is retried once after a pause.
    pub async fn login(&self, apple_id: &str, password: &[u8]) -> Result<LoginState> {
        match self.login_once(apple_id, password).await {
            Err(e) if busy(&e) => {
                tracing::debug!(delay_ms = self.retry_delay.as_millis() as u64, "gsa: busy; retrying the login once");
                tokio::time::sleep(self.retry_delay).await;
                self.login_once(apple_id, password).await
            }
            other => other,
        }
    }

    async fn login_once(&self, apple_id: &str, password: &[u8]) -> Result<LoginState> {
        let user = apple_id.trim().to_lowercase();
        let id = self.identity().await?;
        let anisette = self.anisette.headers().await?;
        let srp = SrpClient::new(Group::gsa());

        let mut init = plist::Dictionary::new();
        init.insert("A2k".into(), plist::Value::Data(srp.a_pub()));
        init.insert("cpd".into(), Self::cpd(&anisette, &id));
        init.insert("o".into(), "init".into());
        init.insert("ps".into(), plist::Value::Array(vec!["s2k".into(), "s2k_fo".into()]));
        init.insert("u".into(), user.clone().into());
        let r = self.gs_post("init", init, &id).await?;
        if let Some(e) = r.get("Status").and_then(plist::Value::as_dictionary).and_then(|s| status_error(s, "init")) {
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
        complete.insert("cpd".into(), Self::cpd(&anisette, &id));
        complete.insert("o".into(), "complete".into());
        complete.insert("u".into(), user.into());
        let r = self.gs_post("complete", complete, &id).await?;
        let status = r.get("Status").and_then(plist::Value::as_dictionary).cloned().unwrap_or_default();
        if let Some(e) = status_error(&status, "complete") {
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
        let id = self.identity().await?;
        let mut pairs: Vec<(String, String)> = vec![
            ("X-Apple-Identity-Token".into(), s.identity_token().to_string()),
            ("X-Apple-App-Info".into(), self.profile.app_info.clone()),
            ("X-Xcode-Version".into(), self.profile.xcode_version.clone()),
            ("X-Mme-Client-Info".into(), id.client_info),
            ("User-Agent".into(), id.user_agent),
            ("Accept-Language".into(), "en-us".into()),
        ];
        if json {
            pairs.push(("Content-Type".into(), "application/json".into()));
            pairs.push(("Accept".into(), "application/json, text/javascript".into()));
        } else {
            // What working clients send (xtool's GrandSlamTwoFactorRequest); the answer is judged by status.
            pairs.push(("Content-Type".into(), "application/x-plist".into()));
            pairs.push(("Accept".into(), "application/x-buddyml".into()));
        }
        pairs.extend(self.anisette.headers().await?.pairs().into_iter().map(|(k, v)| (k.to_owned(), v)));
        tracing::debug!(headers = ?pairs.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), "gsa: second-factor request");
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
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| unavailable(e.without_url().to_string()))?;
        let numbers = parse_trusted_numbers(&String::from_utf8_lossy(&bytes));
        tracing::debug!(
            stage = "trustedNumbers",
            http = status,
            len = bytes.len(),
            kind = crate::body::kind(&bytes),
            numbers = numbers.len(),
            "gsa: response"
        );
        Ok(numbers)
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
        tracing::debug!(stage = "requestCode", http = status, "gsa: response");
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
                let content_type = header_text(resp.headers(), reqwest::header::CONTENT_TYPE);
                let bytes = resp.bytes().await.map_err(|e| unavailable(e.without_url().to_string()))?;
                judge_code(status, &content_type, &bytes, "validate")
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
                let status = resp.status().as_u16();
                let content_type = header_text(resp.headers(), reqwest::header::CONTENT_TYPE);
                let bytes = resp.bytes().await.map_err(|e| unavailable(e.without_url().to_string()))?;
                judge_code(status, &content_type, &bytes, "validateSms")
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
        let id = self.identity().await?;
        req.insert("cpd".into(), Self::cpd(&anisette, &id));
        req.insert("o".into(), "apptokens".into());
        req.insert("t".into(), s.idms_token.as_str().into());
        req.insert("u".into(), s.adsid.clone().into());
        let r = self.gs_post("apptokens", req, &id).await?;
        if let Some(e) =
            r.get("Status").and_then(plist::Value::as_dictionary).and_then(|st| status_error(st, "apptokens"))
        {
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
            ("X-Mme-Client-Info".into(), self.identity().await?.client_info),
        ];
        pairs.extend(self.anisette.headers().await?.pairs().into_iter().map(|(k, v)| (k.to_owned(), v)));
        Ok(pairs)
    }
}

/// A response header as text (empty when absent or not text).
pub(crate) fn header_text(h: &reqwest::header::HeaderMap, name: reqwest::header::HeaderName) -> String {
    h.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned()
}

/// The `ec` an answer carries (plist `Status.ec` or `ec`; JSON `service_errors[0].code`), if any.
fn answer_ec(body: &[u8]) -> Option<i64> {
    if let Some(d) = crate::body::plist_dict(body) {
        let st = d.get("Status").and_then(plist::Value::as_dictionary).unwrap_or(&d);
        return st.get("ec").and_then(as_i64);
    }
    let j = crate::body::json(body)?;
    j.get("service_errors")?.as_array()?.first()?.get("code").and_then(|c| match c {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.as_i64(),
        _ => None,
    })
}

/// Judges a code check by its HTTP status, as working clients do (Apple's success answer is not a plist): 2xx
/// accepts the code unless the body carries a non-zero `ec`; 423/429 are rate limits; other 4xx a wrong code.
pub(crate) fn judge_code(status: u16, content_type: &str, body: &[u8], stage: &'static str) -> Result<()> {
    let ec = answer_ec(body);
    tracing::debug!(
        stage,
        http = status,
        content_type,
        len = body.len(),
        kind = crate::body::kind(body),
        ec = ?ec,
        "gsa: response"
    );
    match (status, ec) {
        (423 | 429, _) | (_, Some(-20209 | -20283)) => Err(rate_limited()),
        (200..=299, None | Some(0)) => Ok(()),
        (200..=299, Some(_)) => Err(bad_code()),
        (400..=499, _) => Err(bad_code()),
        (s, _) => Err(unavailable(format!("Apple answered HTTP {s} to the code check"))),
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
    fn code_checks_are_judged_by_status_whatever_the_body() {
        let buddyml = b"<?xml version=\"1.0\"?><xmlui><page/></xmlui>";
        let mut d = plist::Dictionary::new();
        d.insert("ec".into(), plist::Value::Integer((-21669).into()));
        let mut wrong = Vec::new();
        plist::to_writer_xml(&mut wrong, &plist::Value::Dictionary(d)).expect("plist");
        let code = |s, b: &[u8]| judge_code(s, "", b, "validate").err().map(|e| e.code());
        assert_eq!(code(200, b""), None);
        assert_eq!(code(200, buddyml), None);
        assert_eq!(code(204, b""), None);
        assert_eq!(code(200, &wrong), Some(codes::gsa::BAD_CODE));
        assert_eq!(code(400, buddyml), Some(codes::gsa::BAD_CODE));
        assert_eq!(code(401, b""), Some(codes::gsa::BAD_CODE));
        assert_eq!(code(400, br#"{"service_errors":[{"code":"-21669"}]}"#), Some(codes::gsa::BAD_CODE));
        assert_eq!(code(429, b""), Some(codes::gsa::CODE_RATE_LIMITED));
        assert_eq!(code(503, b""), Some(codes::account::APPLE_SERVICE_UNAVAILABLE));
    }

    #[test]
    fn statuses_map_to_russet_codes() {
        let st = |ec: i64| {
            let mut d = plist::Dictionary::new();
            d.insert("ec".into(), plist::Value::Integer(ec.into()));
            d.insert("em".into(), "Your someone@example.com account".into());
            status_error(&d, "init").map(|e| (e.code(), e.message().to_owned()))
        };
        assert_eq!(st(0), None);
        assert_eq!(st(-22406).map(|e| e.0), Some(codes::gsa::BAD_CREDENTIALS));
        assert_eq!(st(-20209).map(|e| e.0), Some(codes::gsa::LOCKED));
        assert_eq!(st(-36607).map(|e| e.0), Some(codes::gsa::IDENTITY_REJECTED));
        let mismatch = st(-80045).expect("an error");
        assert_eq!(mismatch.0, codes::gsa::IDENTITY_REJECTED);
        assert!(mismatch.1.contains("at init (-80045)"), "{}", mismatch.1);
        assert_eq!(st(-80046).map(|e| e.0), Some(codes::gsa::IDENTITY_REJECTED));
        let other = st(-1).expect("an error");
        assert_eq!(other.0, codes::gsa::PROTOCOL);
        assert!(!other.1.contains("someone@example.com"), "{}", other.1);
    }
}
