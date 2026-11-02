// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! DS2 authentication (SIGN §7.4, SA-18/SA-20): the typed client of this crate talking to developerservices2's
//! `v1` mirror of the public API with an Apple ID session instead of an API key. One surface for paid and free
//! teams: a paid personal team signed in with its Apple ID provisions through here exactly like a free one.
//!
//! [`Ds2Auth`] rewrites each request the way Xcode sends it:
//! - `GET` and `DELETE` become `POST` with `X-HTTP-Method-Override` and the body
//!   `{"urlEncodedQueryParams": "<original query>&teamId=<team>"}`;
//! - `POST` and `PATCH` bodies get `data.attributes.teamId`;
//! - the JSON:API content types, `User-Agent: Xcode` and the session headers ([`Ds2Credentials`]: the Apple ID's
//!   `X-Apple-GS-Token`/`X-Apple-I-Identity-Id`, the Xcode identity and fresh anisette) are added.
//!
//! The credentials come from `russet-gsa` (this crate sits below it); their values are never logged.

use std::sync::Arc;

use async_trait::async_trait;
use russet_core::Result;
use russet_core::error::{Error, codes};
use url::Url;

use crate::client::{ApiFlavor, Auth};

/// developerservices2's service root; the `v1` mirror and the legacy `QH65B2` actions live below it.
pub const DS2_BASE_URL: &str = "https://developerservices2.apple.com/services/";

/// The JSON:API media type DS2 expects.
pub const JSON_API: &str = "application/vnd.api+json";

/// The per-request headers of an Apple ID session (token, identity, Xcode identity, fresh anisette).
#[async_trait]
pub trait Ds2Credentials: Send + Sync {
    async fn headers(&self) -> Result<Vec<(String, String)>>;
}

/// [`Auth`] for developerservices2 with an Apple ID session.
pub struct Ds2Auth {
    base: Url,
    team_id: String,
    creds: Arc<dyn Ds2Credentials>,
}

impl std::fmt::Debug for Ds2Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ds2Auth").field("base", &self.base.as_str()).finish_non_exhaustive()
    }
}

impl Ds2Auth {
    /// `base` is the service root ([`DS2_BASE_URL`], or a fake's); requests act for `team_id`.
    pub fn new(base: Url, team_id: impl Into<String>, creds: Arc<dyn Ds2Credentials>) -> Self {
        Self { base, team_id: team_id.into(), creds }
    }
}

fn internal(msg: impl Into<String>) -> Error {
    Error::new(codes::core::INTERNAL, msg)
}

/// `body` with `data.attributes.teamId` set (a body without `data` is left alone).
pub fn inject_team(mut body: serde_json::Value, team_id: &str) -> serde_json::Value {
    if let Some(data) = body.get_mut("data").and_then(serde_json::Value::as_object_mut) {
        let attrs = data.entry("attributes").or_insert_with(|| serde_json::json!({}));
        if let Some(a) = attrs.as_object_mut() {
            a.insert("teamId".into(), serde_json::Value::String(team_id.to_owned()));
        }
    }
    body
}

/// The `urlEncodedQueryParams` of an overridden GET or DELETE.
pub fn query_params(query: Option<&str>, team_id: &str) -> String {
    let team = format!("teamId={}", url::form_urlencoded::byte_serialize(team_id.as_bytes()).collect::<String>());
    match query.filter(|q| !q.is_empty()) {
        Some(q) => format!("{q}&{team}"),
        None => team,
    }
}

#[async_trait]
impl Auth for Ds2Auth {
    fn base_url(&self) -> &Url {
        &self.base
    }

    fn flavor(&self) -> ApiFlavor {
        ApiFlavor::XcodeDs2 { team_id: self.team_id.clone() }
    }

    async fn authorize(&self, req: &mut reqwest::Request) -> Result<()> {
        let method = req.method().clone();
        if method == reqwest::Method::GET || method == reqwest::Method::DELETE {
            let params = query_params(req.url().query(), &self.team_id);
            let body = serde_json::to_vec(&serde_json::json!({ "urlEncodedQueryParams": params }))
                .map_err(|e| internal(e.to_string()))?;
            req.url_mut().set_query(None);
            *req.method_mut() = reqwest::Method::POST;
            *req.body_mut() = Some(body.into());
            req.headers_mut().insert(
                "X-HTTP-Method-Override",
                reqwest::header::HeaderValue::from_static(if method == reqwest::Method::GET {
                    "GET"
                } else {
                    "DELETE"
                }),
            );
        } else if method == reqwest::Method::POST || method == reqwest::Method::PATCH {
            let original = req.body().and_then(reqwest::Body::as_bytes).map(<[u8]>::to_vec).unwrap_or_default();
            let json: serde_json::Value = if original.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_slice(&original)
                    .map_err(|e| internal(format!("a DS2 request body is not JSON: {e}")))?
            };
            let body = serde_json::to_vec(&inject_team(json, &self.team_id)).map_err(|e| internal(e.to_string()))?;
            *req.body_mut() = Some(body.into());
        }
        let h = req.headers_mut();
        let set = |h: &mut reqwest::header::HeaderMap, k: &str, v: &str| -> Result<()> {
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| internal(format!("invalid DS2 header name {k}")))?;
            let value = reqwest::header::HeaderValue::from_str(v)
                .map_err(|_| internal(format!("the DS2 header {k} has an invalid value")))?;
            h.insert(name, value);
            Ok(())
        };
        set(h, "Accept", JSON_API)?;
        set(h, "Content-Type", JSON_API)?;
        set(h, "User-Agent", "Xcode")?;
        for (k, v) in self.creds.headers().await? {
            set(req.headers_mut(), &k, &v)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed;

    #[async_trait]
    impl Ds2Credentials for Fixed {
        async fn headers(&self) -> Result<Vec<(String, String)>> {
            Ok(vec![("X-Apple-GS-Token".into(), "t".into())])
        }
    }

    #[test]
    fn post_bodies_carry_the_team() {
        let b = inject_team(serde_json::json!({"data": {"type": "bundleIds"}}), "TEAM000001");
        assert_eq!(b["data"]["attributes"]["teamId"], "TEAM000001");
        let b = inject_team(serde_json::json!({"data": {"attributes": {"name": "x"}}}), "T");
        assert_eq!(b["data"]["attributes"]["name"], "x");
        assert_eq!(query_params(Some("limit=200"), "T 1"), "limit=200&teamId=T+1");
        assert_eq!(query_params(None, "T"), "teamId=T");
    }

    #[tokio::test]
    async fn gets_become_overridden_posts() {
        let base = Url::parse("http://127.0.0.1:9/services/").expect("url");
        let auth = Ds2Auth::new(base.clone(), "TEAM000001", Arc::new(Fixed));
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::new();
        let mut req = client.get(base.join("v1/certificates?limit=1").expect("url")).build().expect("request");
        auth.authorize(&mut req).await.expect("authorized");
        assert_eq!(req.method(), reqwest::Method::POST);
        assert_eq!(req.url().query(), None);
        assert_eq!(req.headers().get("X-HTTP-Method-Override").and_then(|v| v.to_str().ok()), Some("GET"));
        assert_eq!(req.headers().get("X-Apple-GS-Token").and_then(|v| v.to_str().ok()), Some("t"));
        let body: serde_json::Value =
            serde_json::from_slice(req.body().and_then(reqwest::Body::as_bytes).unwrap_or_default()).expect("json");
        assert_eq!(body["urlEncodedQueryParams"], "limit=1&teamId=TEAM000001");
        assert_eq!(auth.flavor(), ApiFlavor::XcodeDs2 { team_id: "TEAM000001".into() });
    }
}
