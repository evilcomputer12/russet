// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Machine provisioning against GrandSlam (SIGN §7.5): `lookup` names `midStartProvisioning` and
//! `midFinishProvisioning`; start returns `spim`, ADI turns it into `cpim`, finish returns `ptm`, `tk` and the
//! routing info. This crate performs these requests for both providers (the ADI helper has no network; a v3
//! server only computes).

use base64::Engine as _;
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::net::Endpoints;
use url::Url;

use crate::http::{dict, plist_xml};
use crate::state::AnisetteState;

/// How the client presents itself to GrandSlam (from `gsa-profile.json`, SIGN §7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity {
    /// `X-Mme-Client-Info`.
    pub client_info: String,
    /// `User-Agent`.
    pub user_agent: String,
}

/// The GrandSlam service URLs `lookup` returns, rebased onto the configured GSA origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupUrls {
    pub gs_service: Url,
    pub secondary_auth: Url,
    pub trusted_device_secondary_auth: Url,
    pub validate_code: Url,
    pub mid_start_provisioning: Url,
    pub mid_finish_provisioning: Url,
}

impl LookupUrls {
    /// The well-known paths under `base` (used when `lookup` omits one).
    pub fn defaults(base: &Url) -> Result<Self> {
        let j = |p: &str| base.join(p).map_err(|e| Error::invalid_argument(format!("gsa endpoint: {e}")));
        Ok(Self {
            gs_service: j("grandslam/GsService2")?,
            secondary_auth: j("auth")?,
            trusted_device_secondary_auth: j("auth/verify/trusteddevice")?,
            validate_code: j("grandslam/GsService2/validate")?,
            mid_start_provisioning: j("grandslam/MidService/startMachineProvisioning")?,
            mid_finish_provisioning: j("grandslam/MidService/finishMachineProvisioning")?,
        })
    }
}

/// `url`'s path and query on `base`'s origin: GrandSlam's `lookup` names absolute URLs on gsa.apple.com, and Russet
/// sends every request to the configured endpoint (the fakes of the mocked suites included).
pub fn rebase(base: &Url, url: &str) -> Option<Url> {
    let u = Url::parse(url).or_else(|_| base.join(url)).ok()?;
    let mut out = base.clone();
    out.set_path(u.path());
    out.set_query(u.query());
    Some(out)
}

pub(crate) fn protocol(msg: impl Into<String>) -> Error {
    Error::new(codes::gsa::PROTOCOL, msg).with_hint(
        "Apple changed something Russet relies on, or a proxy altered the answer; try again, and update Russet if it persists.",
    )
}

fn provisioning_failed(msg: impl Into<String>) -> Error {
    Error::new(codes::anisette::PROVISIONING_FAILED, msg)
        .with_hint("Apple refused to provision this machine for sign-in. Try again later; `russet account anisette reset` starts over with a new machine identity.")
}

/// `GET <gsa>/grandslam/GsService2/lookup`, with the well-known defaults for anything missing.
pub async fn lookup(http: &reqwest::Client, base: &Url, id: &ClientIdentity) -> Result<LookupUrls> {
    let mut urls = LookupUrls::defaults(base)?;
    let url = base.join("grandslam/GsService2/lookup").map_err(|e| Error::invalid_argument(e.to_string()))?;
    Endpoints::check(&url)?;
    let resp = http
        .get(url)
        .header("X-Mme-Client-Info", &id.client_info)
        .header(reqwest::header::USER_AGENT, &id.user_agent)
        .header(reqwest::header::ACCEPT, "*/*")
        .send()
        .await
        .map_err(|e| unavailable(format!("GrandSlam lookup failed: {e}")))?;
    let body = resp.bytes().await.map_err(|e| unavailable(e.to_string()))?;
    let Ok(plist::Value::Dictionary(d)) = plist::from_bytes::<plist::Value>(&body) else { return Ok(urls) };
    let Some(plist::Value::Dictionary(u)) = d.get("urls") else { return Ok(urls) };
    let get = |k: &str| u.get(k).and_then(plist::Value::as_string).and_then(|s| rebase(base, s));
    if let Some(v) = get("gsService") {
        urls.gs_service = v;
    }
    if let Some(v) = get("secondaryAuth") {
        urls.secondary_auth = v;
    }
    if let Some(v) = get("trustedDeviceSecondaryAuth") {
        urls.trusted_device_secondary_auth = v;
    }
    if let Some(v) = get("validateCode") {
        urls.validate_code = v;
    }
    if let Some(v) = get("midStartProvisioning") {
        urls.mid_start_provisioning = v;
    }
    if let Some(v) = get("midFinishProvisioning") {
        urls.mid_finish_provisioning = v;
    }
    Ok(urls)
}

pub(crate) fn unavailable(msg: impl Into<String>) -> Error {
    Error::new(codes::account::APPLE_SERVICE_UNAVAILABLE, msg)
        .with_hint("Check the network; Apple's sign-in service may be briefly unavailable.")
        .retryable(true)
}

/// The two provisioning requests.
pub(crate) struct Provisioner<'a> {
    pub http: &'a reqwest::Client,
    pub urls: &'a LookupUrls,
    pub id: &'a ClientIdentity,
}

impl Provisioner<'_> {
    fn request(&self, url: &Url, state: &AnisetteState, body: Vec<u8>) -> reqwest::RequestBuilder {
        self.http
            .post(url.clone())
            .header(reqwest::header::CONTENT_TYPE, "text/x-xml-plist")
            .header(reqwest::header::ACCEPT, "*/*")
            .header(reqwest::header::USER_AGENT, &self.id.user_agent)
            .header("X-Mme-Client-Info", &self.id.client_info)
            .header("X-Mme-Device-Id", state.device_id())
            .header("X-Apple-I-MD-LU", state.md_lu())
            .header("X-Apple-I-Client-Time", crate::client_time_now())
            .header("X-Apple-I-TimeZone", "UTC")
            .header("X-Apple-Locale", "en_US")
            .header("X-Apple-I-SRL-NO", "0")
            .body(body)
    }

    async fn call(&self, url: &Url, state: &AnisetteState, request: plist::Value) -> Result<plist::Dictionary> {
        Endpoints::check(url)?;
        let body = plist_xml(&dict([("Header", dict([])), ("Request", request)]))?;
        let resp = self
            .request(url, state, body)
            .send()
            .await
            .map_err(|e| unavailable(format!("anisette provisioning request failed: {e}")))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| unavailable(e.to_string()))?;
        if status >= 500 || status == 429 {
            return Err(unavailable(format!("GrandSlam answered {status} to anisette provisioning")));
        }
        let v: plist::Value = plist::from_bytes(&bytes)
            .map_err(|_| protocol(format!("unreadable provisioning answer (HTTP {status})")))?;
        let response = v
            .as_dictionary()
            .and_then(|d| d.get("Response"))
            .and_then(plist::Value::as_dictionary)
            .cloned()
            .ok_or_else(|| protocol("the provisioning answer has no Response"))?;
        let status_dict = response.get("Status").and_then(plist::Value::as_dictionary);
        let ec = status_dict.and_then(|s| s.get("ec")).and_then(as_i64).unwrap_or(0);
        if ec != 0 {
            let em = status_dict.and_then(|s| s.get("em")).and_then(plist::Value::as_string).unwrap_or("");
            return Err(provisioning_failed(format!("provisioning refused ({ec}): {em}")));
        }
        Ok(response)
    }

    /// `midStartProvisioning` → `spim`.
    pub async fn start(&self, state: &AnisetteState) -> Result<Vec<u8>> {
        let r = self.call(&self.urls.mid_start_provisioning, state, dict([])).await?;
        b64_field(&r, "spim")
    }

    /// `midFinishProvisioning {cpim}` → `(ptm, tk, rinfo)`.
    pub async fn finish(&self, state: &AnisetteState, cpim: &[u8]) -> Result<(Vec<u8>, Vec<u8>, Option<String>)> {
        let cpim = base64::engine::general_purpose::STANDARD.encode(cpim);
        let r =
            self.call(&self.urls.mid_finish_provisioning, state, dict([("cpim", plist::Value::String(cpim))])).await?;
        let rinfo = r.get("X-Apple-I-MD-RINFO").and_then(|v| match v {
            plist::Value::String(s) => Some(s.clone()),
            plist::Value::Integer(i) => Some(i.to_string()),
            _ => None,
        });
        Ok((b64_field(&r, "ptm")?, b64_field(&r, "tk")?, rinfo))
    }
}

pub(crate) fn as_i64(v: &plist::Value) -> Option<i64> {
    match v {
        plist::Value::Integer(i) => i.as_signed(),
        plist::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn b64_field(d: &plist::Dictionary, key: &str) -> Result<Vec<u8>> {
    match d.get(key) {
        Some(plist::Value::String(s)) => base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .map_err(|_| protocol(format!("provisioning field {key} is not base64"))),
        Some(plist::Value::Data(b)) => Ok(b.clone()),
        _ => Err(protocol(format!("the provisioning answer has no {key}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_urls_are_rebased_onto_the_configured_origin() {
        let base = Url::parse("http://127.0.0.1:4000/").expect("url");
        let u = rebase(&base, "https://gsa.apple.com/grandslam/GsService2?x=1").expect("rebased");
        assert_eq!(u.as_str(), "http://127.0.0.1:4000/grandslam/GsService2?x=1");
        let d = LookupUrls::defaults(&base).expect("defaults");
        assert_eq!(d.validate_code.path(), "/grandslam/GsService2/validate");
    }
}
