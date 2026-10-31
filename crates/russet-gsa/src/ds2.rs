// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! developerservices2 with an Apple ID (SIGN §7.4): the [`Ds2Credentials`] of a signed-in session for
//! `russet_asc::ds2::Ds2Auth` (the `v1` mirror), and the legacy `QH65B2` actions the public API lacks: teams and
//! app groups. Russet never revokes anything here.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russet_asc::ds2::Ds2Credentials;
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::net::Endpoints;
use url::Url;

use crate::client::{GsaClient, header_map, protocol, session_expired, unavailable};
use crate::session::XcodeToken;

/// Xcode's DS2 client id.
pub const CLIENT_ID: &str = "XABBG36SBA";

/// The DS2 credentials of a signed-in Apple ID: its Xcode token plus fresh anisette per request.
pub struct Ds2Session {
    gsa: Arc<GsaClient>,
    token: XcodeToken,
}

impl std::fmt::Debug for Ds2Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ds2Session").finish_non_exhaustive()
    }
}

impl Ds2Session {
    pub fn new(gsa: Arc<GsaClient>, token: XcodeToken) -> Self {
        Self { gsa, token }
    }
}

#[async_trait]
impl Ds2Credentials for Ds2Session {
    async fn headers(&self) -> Result<Vec<(String, String)>> {
        if !self.token.fresh(time::OffsetDateTime::now_utc(), time::Duration::seconds(30)) {
            return Err(session_expired());
        }
        self.gsa.ds2_headers(&self.token).await
    }
}

/// A developer team as `listTeams` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Team {
    pub team_id: String,
    pub name: String,
    /// `Individual`, `Company`, …
    pub kind: String,
    pub status: String,
    /// Membership names (`Apple Developer Program`, `Xcode Free Provisioning Program`, …).
    pub memberships: Vec<String>,
}

/// Paid or free, from the memberships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamTier {
    Paid,
    Free,
}

impl TeamTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Paid => "paid",
            Self::Free => "free",
        }
    }
}

impl Team {
    /// A team is free unless a membership is a developer program (xtool's rule, SIGN §7.4).
    pub fn tier(&self) -> TeamTier {
        let paid = self.memberships.iter().any(|m| {
            let m = m.to_ascii_lowercase();
            m.contains("developer program") && !m.contains("free")
        });
        if paid { TeamTier::Paid } else { TeamTier::Free }
    }
}

/// An app group (`ios/listApplicationGroups`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppGroup {
    pub id: String,
    pub identifier: String,
    pub name: String,
}

/// The legacy `QH65B2` dialect.
pub struct Qh65b2 {
    http: reqwest::Client,
    base: Url,
    creds: Arc<dyn Ds2Credentials>,
}

impl std::fmt::Debug for Qh65b2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Qh65b2").field("base", &self.base.as_str()).finish_non_exhaustive()
    }
}

fn ds2_failed(code: i64, text: &str) -> Error {
    Error::new(
        codes::gsa::DS2_FAILED,
        format!("developer services refused the request ({code}): {}", crate::client::scrub(text)),
    )
    .with_hint(
        "Check the team's status at developer.apple.com (agreements to accept, membership expired), then try again.",
    )
}

impl Qh65b2 {
    /// `base` is the DS2 service root (`russet_asc::ds2::DS2_BASE_URL`, or a fake's).
    pub fn new(base: Url, creds: Arc<dyn Ds2Credentials>) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot build the HTTP client: {e}")))?;
        Ok(Self { http, base, creds })
    }

    /// POSTs a `QH65B2` action and returns the answer (after checking `resultCode`).
    pub async fn action(&self, action: &str, params: plist::Dictionary) -> Result<plist::Dictionary> {
        let url = self
            .base
            .join(&format!("QH65B2/{action}.action?clientId={CLIENT_ID}"))
            .map_err(|e| Error::invalid_argument(e.to_string()))?;
        Endpoints::check(&url)?;
        let mut body = params;
        body.insert("clientId".into(), CLIENT_ID.into());
        body.insert("protocolVersion".into(), "QH65B2".into());
        body.insert("requestId".into(), uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase().into());
        body.insert("userLocale".into(), plist::Value::Array(vec!["en_US".into()]));
        let mut bytes = Vec::new();
        plist::to_writer_xml(&mut bytes, &plist::Value::Dictionary(body))
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode a DS2 request: {e}")))?;
        let mut pairs = vec![
            ("Content-Type".to_owned(), "text/x-xml-plist".to_owned()),
            ("Accept".to_owned(), "text/x-xml-plist".to_owned()),
            ("Accept-Language".to_owned(), "en-us".to_owned()),
            ("User-Agent".to_owned(), "Xcode".to_owned()),
        ];
        pairs.extend(self.creds.headers().await?);
        let resp = self
            .http
            .post(url)
            .headers(header_map(&pairs)?)
            .body(bytes)
            .send()
            .await
            .map_err(|e| unavailable(format!("developer services are unreachable: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| unavailable(e.without_url().to_string()))?;
        if status == 401 || status == 403 {
            return Err(session_expired());
        }
        if status >= 500 {
            return Err(unavailable(format!("developer services answered HTTP {status}")));
        }
        let v: plist::Value = plist::from_bytes(&bytes)
            .map_err(|_| protocol(format!("developer services answered HTTP {status} with no plist")))?;
        let d = v.as_dictionary().cloned().ok_or_else(|| protocol("the DS2 answer is not a dictionary"))?;
        let code = match d.get("resultCode") {
            Some(plist::Value::Integer(i)) => i.as_signed().unwrap_or(-1),
            Some(plist::Value::String(s)) => s.parse().unwrap_or(-1),
            _ => 0,
        };
        if code == 0 {
            return Ok(d);
        }
        let text = d
            .get("userString")
            .or_else(|| d.get("resultString"))
            .and_then(plist::Value::as_string)
            .unwrap_or("no reason given");
        Err(if code == 1100 { session_expired() } else { ds2_failed(code, text) })
    }

    /// `listTeams`.
    pub async fn list_teams(&self) -> Result<Vec<Team>> {
        let d = self.action("listTeams", plist::Dictionary::new()).await?;
        let teams = d.get("teams").and_then(plist::Value::as_array).cloned().unwrap_or_default();
        Ok(teams
            .iter()
            .filter_map(|t| {
                let t = t.as_dictionary()?;
                let s = |k: &str| t.get(k).and_then(plist::Value::as_string).unwrap_or_default().to_owned();
                let memberships = t
                    .get("memberships")
                    .and_then(plist::Value::as_array)
                    .map(|m| {
                        m.iter()
                            .filter_map(|x| x.as_dictionary()?.get("name")?.as_string().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let team_id = s("teamId");
                (!team_id.is_empty()).then(|| Team {
                    team_id,
                    name: s("name"),
                    kind: s("type"),
                    status: s("status"),
                    memberships,
                })
            })
            .collect())
    }

    fn team_params(team_id: &str) -> plist::Dictionary {
        let mut d = plist::Dictionary::new();
        d.insert("teamId".into(), team_id.into());
        d
    }

    /// `ios/listApplicationGroups`.
    pub async fn list_app_groups(&self, team_id: &str) -> Result<Vec<AppGroup>> {
        let d = self.action("ios/listApplicationGroups", Self::team_params(team_id)).await?;
        let groups = d.get("applicationGroupList").and_then(plist::Value::as_array).cloned().unwrap_or_default();
        Ok(groups.iter().filter_map(app_group).collect())
    }

    /// `ios/addApplicationGroup`.
    pub async fn add_app_group(&self, team_id: &str, identifier: &str, name: &str) -> Result<AppGroup> {
        let mut p = Self::team_params(team_id);
        p.insert("identifier".into(), identifier.into());
        p.insert("name".into(), name.into());
        let d = self.action("ios/addApplicationGroup", p).await?;
        d.get("applicationGroup").and_then(app_group).ok_or_else(|| protocol("addApplicationGroup returned no group"))
    }

    /// `ios/assignApplicationGroupToAppId`.
    pub async fn assign_app_groups(&self, team_id: &str, app_id_id: &str, group_ids: &[String]) -> Result<()> {
        let mut p = Self::team_params(team_id);
        p.insert("appIdId".into(), app_id_id.into());
        p.insert(
            "applicationGroups".into(),
            plist::Value::Array(group_ids.iter().map(|g| plist::Value::String(g.clone())).collect()),
        );
        self.action("ios/assignApplicationGroupToAppId", p).await.map(|_| ())
    }
}

fn app_group(v: &plist::Value) -> Option<AppGroup> {
    let d = v.as_dictionary()?;
    let s = |k: &str| d.get(k).and_then(plist::Value::as_string).map(str::to_owned);
    Some(AppGroup { id: s("applicationGroup")?, identifier: s("identifier")?, name: s("name").unwrap_or_default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_follow_the_memberships() {
        let t = |m: &[&str]| Team {
            team_id: "T".into(),
            name: String::new(),
            kind: "Individual".into(),
            status: "active".into(),
            memberships: m.iter().map(|s| (*s).to_owned()).collect(),
        };
        assert_eq!(t(&["Apple Developer Program"]).tier(), TeamTier::Paid);
        assert_eq!(t(&["Xcode Free Provisioning Program"]).tier(), TeamTier::Free);
        assert_eq!(t(&[]).tier(), TeamTier::Free);
    }
}
