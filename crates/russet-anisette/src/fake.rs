// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Deterministic fakes for the mocked suites (feature `fake`; W22 acceptance: `AnisetteFake`): [`AnisetteFake`] is
//! a provider with synthetic headers, [`FakeAdi`] an [`AdiBackend`] that provisions against the GrandSlam fake
//! without Apple's libraries. Neither contains anything of Apple's.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use russet_core::Result;

use crate::local::{AdiBackend, LoadRequest, adi_failed};
use crate::{ADI_DSID, AnisetteHeaders, AnisetteKind, AnisetteProvider, DEFAULT_RINFO};

/// The synthetic `X-Apple-I-MD-M` every fake answers with.
pub const FAKE_MID: &[u8] = b"russet-fake-machine-id";

/// A provider with synthetic, per-request headers.
#[derive(Debug, Default)]
pub struct AnisetteFake {
    counter: AtomicU32,
    resets: AtomicU32,
}

impl AnisetteFake {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many headers were handed out.
    pub fn requests(&self) -> u32 {
        self.counter.load(Ordering::SeqCst)
    }

    pub fn resets(&self) -> u32 {
        self.resets.load(Ordering::SeqCst)
    }
}

/// The client info fakes present: the platform GrandSlam accepts with Android ADI data (a synthetic PC).
pub const FAKE_CLIENT_INFO: &str = "<PC> <Windows;6.2(0,0);9200> <com.apple.AuthKit/1 (com.apple.akd/1.0)>";

/// The akd user agent fakes present.
pub const FAKE_USER_AGENT: &str = "akd/1.0 CFNetwork/808.1.4";

#[async_trait]
impl AnisetteProvider for AnisetteFake {
    fn kind(&self) -> AnisetteKind {
        AnisetteKind::Local
    }

    async fn headers(&self) -> Result<AnisetteHeaders> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        Ok(AnisetteHeaders {
            md: format!("ZmFrZS1vdHAt{n:04}"),
            md_m: "cnVzc2V0LWZha2UtbWFjaGluZS1pZA==".into(),
            md_rinfo: DEFAULT_RINFO.into(),
            md_lu: "0".repeat(64),
            device_id: "00000000-0000-4000-8000-000000000000".into(),
            serial: "0".into(),
            client_time: crate::client_time_now(),
            locale: "en_US".into(),
            time_zone: "UTC".into(),
        })
    }

    async fn identity(&self) -> Result<crate::ClientIdentity> {
        Ok(crate::ClientIdentity { client_info: FAKE_CLIENT_INFO.into(), user_agent: FAKE_USER_AGENT.into() })
    }

    async fn reset(&self) -> Result<()> {
        self.resets.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Debug, Default)]
struct FakeAdiState {
    loaded: Option<LoadRequest>,
    provisioned: bool,
    sessions: u32,
    otps: u32,
}

/// An ADI backend without Apple's libraries: `cpim = "cpim:" ‖ spim`, provisioning ends when `ptm` and `tk` are
/// what the GrandSlam fake derives from that `cpim`, and it writes a synthetic `adi.pb`.
#[derive(Debug, Default)]
pub struct FakeAdi {
    state: Mutex<FakeAdiState>,
}

impl FakeAdi {
    pub fn new() -> Self {
        Self::default()
    }

    /// A backend that starts provisioned (no provisioning requests).
    pub fn provisioned() -> Self {
        Self { state: Mutex::new(FakeAdiState { provisioned: true, ..Default::default() }) }
    }

    pub fn otps(&self) -> u32 {
        self.state.lock().map(|s| s.otps).unwrap_or_default()
    }

    pub fn is_loaded(&self) -> bool {
        self.state.lock().map(|s| s.loaded.is_some()).unwrap_or_default()
    }

    /// The `ptm` the GrandSlam fake answers for `cpim` (and [`FakeAdi`] expects).
    pub fn ptm_for(cpim: &[u8]) -> Vec<u8> {
        [b"ptm:".as_slice(), cpim].concat()
    }

    /// The `tk` the GrandSlam fake answers for `cpim`.
    pub fn tk_for(cpim: &[u8]) -> Vec<u8> {
        [b"tk:".as_slice(), cpim].concat()
    }
}

#[async_trait]
impl AdiBackend for FakeAdi {
    async fn load(&self, req: &LoadRequest) -> Result<()> {
        let mut s = self.state.lock().map_err(|_| adi_failed("poisoned"))?;
        if req.provisioning_path.join("adi.pb").exists() {
            s.provisioned = true;
        }
        s.loaded = Some(req.clone());
        Ok(())
    }

    async fn is_provisioned(&self, dsid: i64) -> Result<bool> {
        let s = self.state.lock().map_err(|_| adi_failed("poisoned"))?;
        Ok(dsid == ADI_DSID && s.provisioned)
    }

    async fn start_provisioning(&self, _dsid: i64, spim: &[u8]) -> Result<(u32, Vec<u8>)> {
        let mut s = self.state.lock().map_err(|_| adi_failed("poisoned"))?;
        if s.loaded.is_none() {
            return Err(adi_failed("not loaded"));
        }
        s.sessions += 1;
        Ok((s.sessions, [b"cpim:".as_slice(), spim].concat()))
    }

    async fn end_provisioning(&self, session: u32, ptm: &[u8], tk: &[u8]) -> Result<()> {
        let mut s = self.state.lock().map_err(|_| adi_failed("poisoned"))?;
        let ok = ptm.strip_prefix(b"ptm:").zip(tk.strip_prefix(b"tk:")).is_some_and(|(a, b)| a == b);
        if session != s.sessions || !ok {
            return Err(adi_failed("ADI failed with status -45054"));
        }
        let path = s.loaded.as_ref().map(|l| l.provisioning_path.join("adi.pb"));
        if let Some(p) = path {
            std::fs::write(&p, b"russet-fake-adi-pb").map_err(|e| adi_failed(e.to_string()))?;
        }
        s.provisioned = true;
        Ok(())
    }

    async fn request_otp(&self, _dsid: i64) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut s = self.state.lock().map_err(|_| adi_failed("poisoned"))?;
        if !s.provisioned {
            return Err(
                adi_failed("ADI reports this machine is logged out (-45061); it provisions again").retryable(true)
            );
        }
        s.otps += 1;
        Ok((FAKE_MID.to_vec(), format!("fake-otp-{}", s.otps).into_bytes()))
    }

    async fn erase(&self, _dsid: i64) -> Result<()> {
        let mut s = self.state.lock().map_err(|_| adi_failed("poisoned"))?;
        s.provisioned = false;
        Ok(())
    }
}
