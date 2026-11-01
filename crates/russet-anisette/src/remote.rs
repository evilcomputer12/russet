// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `RemoteV3` (opt-in, SIGN §7.5): an anisette-v3 server the user runs. `GET /v3/client_info`; the websocket
//! `/v3/provisioning_session` (`GiveIdentifier → GiveStartProvisioningData → GiveEndProvisioningData →
//! ProvisioningSuccess{adi_pb}`), where this side performs Apple's provisioning requests; then
//! `POST /v3/get_headers {identifier, adi_pb}` per request. The server's operator receives this machine's ADI
//! identity, which the UI says; Russet configures no default server.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use futures::{SinkExt as _, StreamExt as _};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::net::Endpoints;
use russet_secrets::SecretStore;
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::provisioning::{ClientIdentity, Provisioner, lookup};
use crate::state::AnisetteState;
use crate::{AnisetteHeaders, AnisetteKind, AnisetteProvider};

fn remote_failed(msg: impl Into<String>) -> Error {
    Error::new(codes::anisette::REMOTE_FAILED, msg)
        .with_hint("Check that the anisette-v3 server is running and reachable, or sign in with local anisette.")
        .retryable(true)
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(s: &str, what: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|_| remote_failed(format!("the anisette server sent a {what} that is not base64")))
}

/// The secret name of a remote server's state: `remote-<12 hex of SHA-256(url)>`.
pub fn state_name(url: &Url) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, url.as_str().as_bytes());
    let hex: String = d.as_ref().iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("remote-{hex}")
}

#[derive(Debug, Deserialize)]
struct ClientInfo {
    client_info: String,
    #[serde(default)]
    user_agent: Option<String>,
}

/// An anisette-v3 server.
pub struct RemoteV3 {
    url: Url,
    http: reqwest::Client,
    gsa_http: reqwest::Client,
    gsa: Url,
    fallback: ClientIdentity,
    secrets: Arc<dyn SecretStore>,
    inner: tokio::sync::Mutex<(Option<ClientIdentity>, Option<AnisetteState>)>,
}

impl std::fmt::Debug for RemoteV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteV3").field("url", &self.url.as_str()).finish_non_exhaustive()
    }
}

impl RemoteV3 {
    /// A provider for the server at `url`; `gsa_http` reaches GrandSlam at `gsa` for the provisioning requests,
    /// presenting the server's client identity (else `fallback`, the GSA profile's).
    pub fn new(
        url: Url,
        secrets: Arc<dyn SecretStore>,
        gsa_http: reqwest::Client,
        gsa: Url,
        fallback: ClientIdentity,
    ) -> Result<Self> {
        if !matches!(url.scheme(), "https" | "http") {
            return Err(Error::invalid_argument(format!(
                "the anisette server URL must be http(s), not {}",
                url.scheme()
            )));
        }
        if url.scheme() == "http" && !russet_core::net::is_loopback(&url) {
            return Err(Error::invalid_argument("a remote anisette server must use https (http only on loopback)")
                .with_hint("The server learns this machine's ADI identity; never send it over plain http."));
        }
        let http = crate::http::plain_client(Duration::from_secs(30))?;
        Ok(Self { url, http, gsa_http, gsa, fallback, secrets, inner: tokio::sync::Mutex::new((None, None)) })
    }

    fn join(&self, path: &str) -> Result<Url> {
        let mut base = self.url.clone();
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        base.join(path).map_err(|e| Error::invalid_argument(e.to_string()))
    }

    async fn server_identity(&self, cached: &mut Option<ClientIdentity>) -> Result<ClientIdentity> {
        if let Some(c) = cached.clone() {
            return Ok(c);
        }
        let url = self.join("v3/client_info")?;
        Endpoints::check(&url)?;
        let info: ClientInfo = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| remote_failed(format!("the anisette server is unreachable: {e}")))?
            .json()
            .await
            .map_err(|_| remote_failed("the anisette server's client_info is unreadable"))?;
        let id = ClientIdentity {
            client_info: info.client_info,
            user_agent: info.user_agent.unwrap_or_else(|| self.fallback.user_agent.clone()),
        };
        *cached = Some(id.clone());
        Ok(id)
    }

    async fn provision(&self, id: &ClientIdentity, state: &mut AnisetteState) -> Result<()> {
        let urls = lookup(&self.gsa_http, &self.gsa, id).await?;
        let p = Provisioner { http: &self.gsa_http, urls: &urls, id };
        let mut ws_url = self.join("v3/provisioning_session")?;
        let scheme = if ws_url.scheme() == "https" { "wss" } else { "ws" };
        let _ = ws_url.set_scheme(scheme);
        let mut check = ws_url.clone();
        let _ = check.set_scheme(if scheme == "wss" { "https" } else { "http" });
        Endpoints::check(&check)?;
        let (mut ws, _) =
            tokio::time::timeout(Duration::from_secs(30), tokio_tungstenite::connect_async(ws_url.as_str()))
                .await
                .map_err(|_| remote_failed("the anisette server's provisioning session timed out"))?
                .map_err(|e| remote_failed(format!("cannot open the provisioning session: {e}")))?;
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(60), ws.next())
                .await
                .map_err(|_| remote_failed("the anisette server stopped answering"))?
                .ok_or_else(|| remote_failed("the anisette server closed the provisioning session"))?
                .map_err(|e| remote_failed(format!("provisioning session: {e}")))?;
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => return Err(remote_failed("the anisette server closed the provisioning session")),
                _ => continue,
            };
            let v: serde_json::Value =
                serde_json::from_str(&text).map_err(|_| remote_failed("unreadable provisioning message"))?;
            let reply = match v["result"].as_str().unwrap_or_default() {
                "GiveIdentifier" => serde_json::json!({"identifier": b64(&state.identifier())}),
                "GiveStartProvisioningData" => serde_json::json!({"spim": b64(&p.start(state).await?)}),
                "GiveEndProvisioningData" => {
                    let cpim = unb64(v["cpim"].as_str().unwrap_or_default(), "cpim")?;
                    let (ptm, tk, rinfo) = p.finish(state, &cpim).await?;
                    if rinfo.is_some() {
                        state.rinfo = rinfo;
                    }
                    serde_json::json!({"ptm": b64(&ptm), "tk": b64(&tk)})
                }
                "ProvisioningSuccess" => {
                    let pb = unb64(v["adi_pb"].as_str().unwrap_or_default(), "adi_pb")?;
                    state.set_adi_pb(&pb);
                    let _ = ws.close(None).await;
                    return Ok(());
                }
                other => {
                    let detail = v["message"].as_str().unwrap_or_default();
                    return Err(remote_failed(format!("the anisette server ended provisioning: {other} {detail}")));
                }
            };
            ws.send(Message::Text(reply.to_string().into()))
                .await
                .map_err(|e| remote_failed(format!("provisioning session: {e}")))?;
        }
    }
}

#[async_trait]
impl AnisetteProvider for RemoteV3 {
    fn kind(&self) -> AnisetteKind {
        AnisetteKind::Remote { url: self.url.clone() }
    }

    async fn headers(&self) -> Result<AnisetteHeaders> {
        let mut inner = self.inner.lock().await;
        let id = self.server_identity(&mut inner.0).await?;
        let name = state_name(&self.url);
        let mut state = match inner.1.clone() {
            Some(s) => s,
            None => AnisetteState::load_or_create(&*self.secrets, &name).await?,
        };
        if state.adi_pb.is_some() && state.client_info.as_deref() != Some(id.client_info.as_str()) {
            // The server changed its identity: its old provisioning would be presented under another platform.
            state.adi_pb = None;
        }
        if state.adi_pb.is_none() {
            self.provision(&id, &mut state).await?;
            state.client_info = Some(id.client_info.clone());
            state.save(&*self.secrets, &name).await?;
        }
        let url = self.join("v3/get_headers")?;
        Endpoints::check(&url)?;
        let v: serde_json::Value = self
            .http
            .post(url)
            .json(&serde_json::json!({"identifier": b64(&state.identifier()), "adi_pb": state.adi_pb.clone().unwrap_or_default()}))
            .send()
            .await
            .map_err(|e| remote_failed(format!("the anisette server is unreachable: {e}")))?
            .json()
            .await
            .map_err(|_| remote_failed("the anisette server's headers are unreadable"))?;
        if v["result"] != "Headers" {
            // A server that lost the identity: provision again on the next request.
            state.adi_pb = None;
            state.save(&*self.secrets, &name).await?;
            inner.1 = Some(state);
            return Err(remote_failed(format!(
                "the anisette server refused headers: {}",
                v["message"].as_str().unwrap_or("no reason given")
            )));
        }
        let field = |k: &str| v[k].as_str().map(str::to_owned);
        let headers = AnisetteHeaders {
            md: field("X-Apple-I-MD").ok_or_else(|| remote_failed("the anisette server sent no X-Apple-I-MD"))?,
            md_m: field("X-Apple-I-MD-M").ok_or_else(|| remote_failed("the anisette server sent no X-Apple-I-MD-M"))?,
            md_rinfo: field("X-Apple-I-MD-RINFO").unwrap_or_else(|| state.rinfo()),
            md_lu: state.md_lu(),
            device_id: state.device_id(),
            serial: "0".into(),
            client_time: crate::client_time_now(),
            locale: "en_US".into(),
            time_zone: "UTC".into(),
        };
        inner.1 = Some(state);
        Ok(headers)
    }

    async fn identity(&self) -> Result<ClientIdentity> {
        let mut inner = self.inner.lock().await;
        self.server_identity(&mut inner.0).await
    }

    async fn reset(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let name = state_name(&self.url);
        let mut state = AnisetteState::load_or_create(&*self.secrets, &name).await?;
        state.adi_pb = None;
        state.save(&*self.secrets, &name).await?;
        inner.1 = None;
        Ok(())
    }
}
