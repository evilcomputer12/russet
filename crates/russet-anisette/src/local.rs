// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Local ADI (SIGN §7.5): Apple's libraries run in the sandboxed `russet-adi-host` child, which this module drives
//! over JSON Lines ([`ProcessBackend`]); provisioning HTTP happens here, never in the child.
//!
//! `adi.pb`, which the library writes into its provisioning directory, is mirrored into the secret store after
//! provisioning (`AnisetteState`) and written back into the 0700 directory before the helper loads, so a wiped
//! data directory does not lose the machine identity Apple trusted after two-factor sign-in.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_secrets::SecretStore;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use url::Url;

use crate::libs::LibrarySet;
use crate::provisioning::{ClientIdentity, Provisioner, lookup};
use crate::state::AnisetteState;
use crate::{ADI_DSID, AnisetteHeaders, AnisetteKind, AnisetteProvider};

/// The secret name of the local provider's state.
pub const STATE_NAME: &str = "local";

/// What the helper loads (`{"op":"load", …}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRequest {
    pub lib_dir: PathBuf,
    pub android_id: String,
    pub provisioning_path: PathBuf,
}

/// The ADI calls (SIGN §7.5): the helper process, or `FakeAdi` in the mocked suites.
#[async_trait]
pub trait AdiBackend: Send + Sync {
    async fn load(&self, req: &LoadRequest) -> Result<()>;
    async fn is_provisioned(&self, dsid: i64) -> Result<bool>;
    /// `spim` → `(session, cpim)`.
    async fn start_provisioning(&self, dsid: i64, spim: &[u8]) -> Result<(u32, Vec<u8>)>;
    async fn end_provisioning(&self, session: u32, ptm: &[u8], tk: &[u8]) -> Result<()>;
    /// `(mid, otp)`: `X-Apple-I-MD-M` and `X-Apple-I-MD`.
    async fn request_otp(&self, dsid: i64) -> Result<(Vec<u8>, Vec<u8>)>;
    async fn erase(&self, dsid: i64) -> Result<()>;
}

/// One request line of the helper protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum HostRequest {
    #[serde(rename_all = "camelCase")]
    Load {
        lib_dir: String,
        android_id: String,
        provisioning_path: String,
    },
    IsProvisioned {
        dsid: i64,
    },
    StartProvisioning {
        dsid: i64,
        spim: String,
    },
    EndProvisioning {
        session: u32,
        ptm: String,
        tk: String,
    },
    RequestOtp {
        dsid: i64,
    },
    Erase {
        dsid: i64,
    },
}

/// One answer line of the helper protocol.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provisioned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpim: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otp: Option<String>,
    /// ADI's status code on failure (`-45061`: logged out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adi_code: Option<i64>,
    /// A loader or protocol failure (never secret material).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub(crate) fn adi_failed(msg: impl Into<String>) -> Error {
    Error::new(codes::anisette::ADI_FAILED, msg).with_hint(
        "The anisette helper failed. `russet account anisette reset` erases the provisioning; if Apple's libraries changed, download them again.",
    )
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(s: Option<&str>, what: &str) -> Result<Vec<u8>> {
    s.and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .ok_or_else(|| adi_failed(format!("the anisette helper answered without {what}")))
}

struct Child {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
}

/// The `russet-adi-host` child, spawned on first use and again after it dies.
pub struct ProcessBackend {
    exe: PathBuf,
    libs: PathBuf,
    state: PathBuf,
    child: tokio::sync::Mutex<Option<Child>>,
    last_load: tokio::sync::Mutex<Option<LoadRequest>>,
}

impl std::fmt::Debug for ProcessBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessBackend").field("exe", &self.exe).finish_non_exhaustive()
    }
}

/// The helper next to the running executable (`russet-adi-host`), unless `RUSSET_ADI_HOST` names one.
pub fn default_host_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RUSSET_ADI_HOST").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) { "russet-adi-host.exe" } else { "russet-adi-host" };
    [dir.join(name), dir.parent()?.join(name)].into_iter().find(|p| p.is_file())
}

impl ProcessBackend {
    /// `exe` sandboxed to read `libs` and write `state` (the provisioning directory).
    pub fn new(exe: PathBuf, libs: &Path, state: &Path) -> Self {
        Self {
            exe,
            libs: libs.to_path_buf(),
            state: state.to_path_buf(),
            child: tokio::sync::Mutex::new(None),
            last_load: tokio::sync::Mutex::new(None),
        }
    }

    fn spawn(&self) -> Result<Child> {
        let mut child = tokio::process::Command::new(&self.exe)
            .arg("--libs")
            .arg(&self.libs)
            .arg("--state")
            .arg(&self.state)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| adi_failed(format!("cannot start {}: {e}", self.exe.display())))?;
        let stdin = child.stdin.take().ok_or_else(|| adi_failed("the anisette helper has no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| adi_failed("the anisette helper has no stdout"))?;
        Ok(Child { child, stdin, stdout: BufReader::new(stdout).lines() })
    }

    async fn call(&self, req: &HostRequest) -> Result<HostResponse> {
        let mut guard = self.child.lock().await;
        let respawned = guard.is_none();
        if respawned {
            *guard = Some(self.spawn()?);
        }
        // A fresh child has nothing loaded: replay the last load first.
        if respawned
            && !matches!(req, HostRequest::Load { .. })
            && let Some(load) = self.last_load.lock().await.clone()
            && let Some(c) = guard.as_mut()
        {
            let r = exchange(c, &load_request(&load)).await;
            if !r.as_ref().is_ok_and(|r| r.ok) {
                *guard = None;
                return Err(adi_failed("the anisette helper could not load Apple's libraries again"));
            }
        }
        let Some(c) = guard.as_mut() else { return Err(adi_failed("the anisette helper is not running")) };
        match exchange(c, req).await {
            Ok(r) => Ok(r),
            Err(e) => {
                if let Some(mut c) = guard.take() {
                    let _ = c.child.start_kill();
                }
                Err(e)
            }
        }
    }

    async fn ok(&self, req: &HostRequest) -> Result<HostResponse> {
        let r = self.call(req).await?;
        if r.ok {
            return Ok(r);
        }
        Err(match (r.adi_code, r.error) {
            (Some(-45061), _) => {
                adi_failed("ADI reports this machine is logged out (-45061); it provisions again").retryable(true)
            }
            (Some(code), _) => adi_failed(format!("ADI failed with status {code}")),
            (None, Some(e)) => adi_failed(format!("the anisette helper failed: {e}")),
            (None, None) => adi_failed("the anisette helper failed"),
        })
    }
}

fn load_request(l: &LoadRequest) -> HostRequest {
    HostRequest::Load {
        lib_dir: l.lib_dir.to_string_lossy().into_owned(),
        android_id: l.android_id.clone(),
        provisioning_path: l.provisioning_path.to_string_lossy().into_owned(),
    }
}

async fn exchange(c: &mut Child, req: &HostRequest) -> Result<HostResponse> {
    let mut line = serde_json::to_vec(req).map_err(|e| adi_failed(e.to_string()))?;
    line.push(b'\n');
    let io = async {
        c.stdin.write_all(&line).await?;
        c.stdin.flush().await?;
        c.stdout.next_line().await
    };
    let answer = tokio::time::timeout(Duration::from_secs(60), io)
        .await
        .map_err(|_| adi_failed("the anisette helper did not answer within 60 s"))?
        .map_err(|e| adi_failed(format!("talking to the anisette helper failed: {e}")))?
        .ok_or_else(|| adi_failed("the anisette helper exited"))?;
    serde_json::from_str(&answer).map_err(|_| adi_failed("the anisette helper answered something unreadable"))
}

#[async_trait]
impl AdiBackend for ProcessBackend {
    async fn load(&self, req: &LoadRequest) -> Result<()> {
        *self.last_load.lock().await = Some(req.clone());
        self.ok(&load_request(req)).await.map(|_| ())
    }

    async fn is_provisioned(&self, dsid: i64) -> Result<bool> {
        Ok(self.ok(&HostRequest::IsProvisioned { dsid }).await?.provisioned.unwrap_or(false))
    }

    async fn start_provisioning(&self, dsid: i64, spim: &[u8]) -> Result<(u32, Vec<u8>)> {
        let r = self.ok(&HostRequest::StartProvisioning { dsid, spim: b64(spim) }).await?;
        let session = r.session.ok_or_else(|| adi_failed("the anisette helper answered without a session"))?;
        Ok((session, unb64(r.cpim.as_deref(), "cpim")?))
    }

    async fn end_provisioning(&self, session: u32, ptm: &[u8], tk: &[u8]) -> Result<()> {
        self.ok(&HostRequest::EndProvisioning { session, ptm: b64(ptm), tk: b64(tk) }).await.map(|_| ())
    }

    async fn request_otp(&self, dsid: i64) -> Result<(Vec<u8>, Vec<u8>)> {
        let r = self.ok(&HostRequest::RequestOtp { dsid }).await?;
        Ok((unb64(r.mid.as_deref(), "mid")?, unb64(r.otp.as_deref(), "otp")?))
    }

    async fn erase(&self, dsid: i64) -> Result<()> {
        self.ok(&HostRequest::Erase { dsid }).await.map(|_| ())
    }
}

struct LocalState {
    state: Option<AnisetteState>,
    loaded: bool,
}

/// The local ADI provider.
pub struct LocalAdi {
    backend: Arc<dyn AdiBackend>,
    secrets: Arc<dyn SecretStore>,
    http: reqwest::Client,
    gsa: Url,
    id: ClientIdentity,
    libs: LibrarySet,
    adi_dir: PathBuf,
    inner: tokio::sync::Mutex<LocalState>,
}

impl std::fmt::Debug for LocalAdi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalAdi").field("apk_version", &self.libs.apk_version).finish_non_exhaustive()
    }
}

/// `<data>/anisette/adi`: the provisioning directory the helper may write.
pub fn adi_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("anisette").join("adi")
}

impl LocalAdi {
    /// A provider over `backend` for the libraries `libs`; `http` reaches GrandSlam at `gsa` (Apple Root CA only).
    pub fn new(
        libs: LibrarySet,
        backend: Arc<dyn AdiBackend>,
        secrets: Arc<dyn SecretStore>,
        http: reqwest::Client,
        gsa: Url,
        id: ClientIdentity,
        adi_dir: PathBuf,
    ) -> Self {
        Self {
            backend,
            secrets,
            http,
            gsa,
            id,
            libs,
            adi_dir,
            inner: tokio::sync::Mutex::new(LocalState { state: None, loaded: false }),
        }
    }

    /// Whether this machine holds ADI provisioning (no helper is started).
    pub async fn provisioned_offline(secrets: &dyn SecretStore) -> bool {
        AnisetteState::load(secrets, STATE_NAME).await.ok().flatten().is_some_and(|s| s.adi_pb.is_some())
    }

    async fn ready(&self, inner: &mut LocalState) -> Result<AnisetteState> {
        let mut state = match inner.state.clone() {
            Some(s) => s,
            None => AnisetteState::load_or_create(&*self.secrets, STATE_NAME).await?,
        };
        if !inner.loaded {
            crate::http::create_private_dir(&self.adi_dir)?;
            let pb = self.adi_dir.join("adi.pb");
            if let Some(bytes) = state.adi_pb_bytes()
                && !pb.exists()
            {
                crate::http::write_private(&pb, &bytes)?;
            }
            self.backend
                .load(&LoadRequest {
                    lib_dir: self.libs.dir.clone(),
                    android_id: state.android_id(),
                    provisioning_path: self.adi_dir.clone(),
                })
                .await?;
            inner.loaded = true;
        }
        if !self.backend.is_provisioned(ADI_DSID).await? {
            self.provision(&mut state).await?;
        }
        inner.state = Some(state.clone());
        Ok(state)
    }

    async fn provision(&self, state: &mut AnisetteState) -> Result<()> {
        let urls = lookup(&self.http, &self.gsa, &self.id).await?;
        let p = Provisioner { http: &self.http, urls: &urls, id: &self.id };
        let spim = p.start(state).await?;
        let (session, cpim) = self.backend.start_provisioning(ADI_DSID, &spim).await?;
        let (ptm, tk, rinfo) = p.finish(state, &cpim).await?;
        self.backend.end_provisioning(session, &ptm, &tk).await?;
        if let Ok(pb) = std::fs::read(self.adi_dir.join("adi.pb")) {
            state.set_adi_pb(&pb);
        }
        state.rinfo = rinfo.or(state.rinfo.take());
        state.save(&*self.secrets, STATE_NAME).await?;
        tracing::info!("anisette: this machine is provisioned for local ADI");
        Ok(())
    }
}

#[async_trait]
impl AnisetteProvider for LocalAdi {
    fn kind(&self) -> AnisetteKind {
        AnisetteKind::Local
    }

    async fn headers(&self) -> Result<AnisetteHeaders> {
        let mut inner = self.inner.lock().await;
        let state = self.ready(&mut inner).await?;
        let (mid, otp) = match self.backend.request_otp(ADI_DSID).await {
            Ok(v) => v,
            Err(e) if e.is_retryable() => {
                // -45061: provisioning was lost; provision once more.
                let mut s = state.clone();
                self.provision(&mut s).await?;
                inner.state = Some(s);
                self.backend.request_otp(ADI_DSID).await?
            }
            Err(e) => return Err(e),
        };
        let state = inner.state.clone().unwrap_or(state);
        Ok(AnisetteHeaders {
            md: b64(&otp),
            md_m: b64(&mid),
            md_rinfo: state.rinfo(),
            md_lu: state.md_lu(),
            device_id: state.device_id(),
            serial: "0".into(),
            client_time: crate::client_time_now(),
            locale: "en_US".into(),
            time_zone: "UTC".into(),
        })
    }

    async fn client_info(&self) -> Result<String> {
        Ok(self.id.client_info.clone())
    }

    async fn reset(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if inner.loaded {
            let _ = self.backend.erase(ADI_DSID).await;
        }
        let _ = std::fs::remove_file(self.adi_dir.join("adi.pb"));
        let mut state = AnisetteState::load_or_create(&*self.secrets, STATE_NAME).await?;
        state.adi_pb = None;
        state.rinfo = None;
        state.save(&*self.secrets, STATE_NAME).await?;
        inner.state = None;
        inner.loaded = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_format_matches_the_design() {
        let r = HostRequest::Load {
            lib_dir: "/l".into(),
            android_id: "0123456789abcdef".into(),
            provisioning_path: "/p".into(),
        };
        let v = serde_json::to_value(&r).expect("json");
        assert_eq!(
            v,
            serde_json::json!({"op":"load","libDir":"/l","androidId":"0123456789abcdef","provisioningPath":"/p"})
        );
        let v = serde_json::to_value(HostRequest::RequestOtp { dsid: -2 }).expect("json");
        assert_eq!(v, serde_json::json!({"op":"requestOtp","dsid":-2}));
        let e: HostResponse = serde_json::from_str(r#"{"ok":false,"adiCode":-45061}"#).expect("parses");
        assert_eq!(e.adi_code, Some(-45061));
    }
}
