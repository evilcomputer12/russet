// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Identity, pairing, Developer Mode and readiness (device.md §6.1–§6.2; DEV-05).
//!
//! Identity is read lazily through lockdown (with a session when the pair record works, else the public subset),
//! pairing follows idevice's `pair` flow (host id, BUID from the mux, Trust dialog, `SavePairRecord`), and
//! Developer Mode comes from the AMFI lockdown service.

use std::time::Duration;

use idevice::Idevice;
use idevice::services::amfi::AmfiClient;
use russet_core::Result;
use russet_core::error::codes;
use serde_json::{Map, Value, json};

use crate::ddi::DdiSource;
use crate::error::{device_error, from_idevice};
use crate::io::FacadeCall;
use crate::io::facade::{self, PairingStatus, TunnelStatusRecord};
use crate::manager::DeviceHandle;
use crate::model::{DeviceInfo, DeviceState, Identity, Readiness, Tri, TunnelState};

/// The lockdown label Russet uses.
pub const LABEL: &str = "russet";

/// Identity keys read with a session (device.md §6.1).
pub const IDENTITY_KEYS: &[&str] = &[
    "DeviceName",
    "ProductType",
    "ProductVersion",
    "BuildVersion",
    "DeviceClass",
    "HardwareModel",
    "CPUArchitecture",
    "ChipID",
];

/// Extra keys `device.info` shows. Serial numbers, ECIDs, phone numbers and network addresses are never read.
pub const INFO_KEYS: &[&str] = &["ProductName", "ActivationState", "PasswordProtected", "HardwarePlatform"];

/// Builds an [`Identity`] from `lockdown.values`.
pub fn identity_from_values(values: &Map<String, Value>) -> Identity {
    let s = |k: &str| values.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    Identity {
        name: s("DeviceName"),
        product_type: s("ProductType"),
        os_version: s("ProductVersion"),
        build: s("BuildVersion"),
        device_class: s("DeviceClass"),
        hardware_model: s("HardwareModel"),
        cpu_architecture: s("CPUArchitecture"),
        chip_id: values.get("ChipID").and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok()),
    }
}

/// Lockdown values through `lockdown.values`, or through `lockdown.get_value` (the whole domain, then the wanted
/// keys) when the connector does not answer the former (russet-testkit's scripted backend, QA §3.5.2).
async fn values(h: &DeviceHandle, keys: &[&str], domain: Option<&str>, session: bool) -> Result<Map<String, Value>> {
    let call = FacadeCall::new(
        facade::LOCKDOWN_VALUES,
        json!({ "udid": h.udid().as_str(), "keys": keys, "domain": domain, "session": session }),
    );
    match h.conn().call(call).await {
        Ok(v) => Ok(v.get("values").and_then(|v| v.as_object()).cloned().unwrap_or_default()),
        Err(e) if crate::io::unanswered(&e) => {
            let all = h
                .conn()
                .call(FacadeCall::new(
                    facade::LOCKDOWN_GET_VALUE,
                    json!({ "udid": h.udid().as_str(), "domain": domain }),
                ))
                .await?;
            let all = all.as_object().cloned().unwrap_or_default();
            Ok(keys.iter().filter_map(|k| all.get(*k).map(|v| ((*k).to_owned(), v.clone()))).collect())
        }
        Err(e) => Err(e),
    }
}

impl DeviceHandle {
    /// Reads and caches the identity once (single-flight); later calls return at once.
    pub async fn ensure_identity(&self) -> Result<()> {
        if self.actor.cached().identity.is_some() {
            return Ok(());
        }
        let _guard = self.actor.identity_lock.lock().await;
        if self.actor.cached().identity.is_some() {
            return Ok(());
        }
        let outcome = values(self, IDENTITY_KEYS, None, true).await;
        let (identity, state) = match outcome {
            Ok(v) => (identity_from_values(&v), DeviceState::Ready),
            Err(e) => {
                let state = match e.code() {
                    c if c == codes::device::NOT_PAIRED || c == codes::device::UNTRUSTED => DeviceState::NeedsTrust,
                    c if c == codes::device::TRUST_PENDING => DeviceState::TrustPending,
                    _ => DeviceState::Unavailable { error: e.to_data() },
                };
                // Unpaired devices still answer the public keys without a session.
                let public = values(self, IDENTITY_KEYS, None, false).await.unwrap_or_default();
                if public.is_empty() && matches!(state, DeviceState::Unavailable { .. }) {
                    self.actor.update(|c| c.state = state);
                    self.changed();
                    return Err(e);
                }
                (identity_from_values(&public), state)
            }
        };
        self.actor.update(|c| {
            c.identity = Some(identity);
            c.state = state;
        });
        self.changed();
        Ok(())
    }

    /// The lock hint (device.md F9): `yes` after an operation reported "Locked", `no` after the next successful
    /// launch, `unknown` before either (iOS 27 cannot be asked).
    pub fn lock_hint(&self) -> Tri {
        self.actor.cached().locked
    }

    /// Records that an operation reported the device locked (or that a launch succeeded). Summaries carry the hint
    /// inside `readiness`; when no client read readiness yet, a "locked" report reads it once in the background, so
    /// `device.list` and `device.changed` show the hint.
    pub(crate) fn set_locked(&self, locked: Tri) {
        let before = self.actor.cached();
        if before.locked != locked {
            self.actor.update(|c| c.locked = locked);
            self.changed();
            if locked == Tri::Yes
                && before.readiness.is_none()
                && let Ok(rt) = tokio::runtime::Handle::try_current()
            {
                let h = self.clone();
                rt.spawn(async move {
                    if let Err(e) = h.readiness(true).await {
                        tracing::debug!(error = %e.message(), "readiness after a lock report failed");
                    }
                });
            }
        }
    }

    /// Marks the lock hint from an error, then passes the error on.
    pub(crate) fn note_error(&self, e: russet_core::Error) -> russet_core::Error {
        if e.code() == codes::device::LOCKED {
            self.set_locked(Tri::Yes);
        }
        e
    }

    /// Developer Mode through the AMFI lockdown service (`action 3`), falling back to the lockdown value.
    pub async fn developer_mode(&self) -> Result<Tri> {
        match self.conn().lockdown_service(self.udid(), "com.apple.amfi.lockdown").await {
            Ok(stream) => {
                let mut amfi = AmfiClient::new(Idevice::new(stream, LABEL));
                match amfi.get_developer_mode_status().await {
                    Ok(b) => return Ok(Tri::from(b)),
                    Err(e) => tracing::debug!(error = %e, "amfi status failed; trying the lockdown value"),
                }
            }
            Err(e) => tracing::debug!(error = %e.message(), "amfi service failed; trying the lockdown value"),
        }
        let v = values(self, &["DeveloperModeStatus"], Some("com.apple.security.mac.amfi"), true).await?;
        Ok(match v.get("DeveloperModeStatus").and_then(|v| v.as_bool()) {
            Some(b) => Tri::from(b),
            None => Tri::Unknown,
        })
    }

    /// Reveals the Developer Mode toggle in Settings (AMFI `action 0`; works without a passcode).
    pub async fn reveal_developer_mode(&self) -> Result<()> {
        let stream = self.conn().lockdown_service(self.udid(), "com.apple.amfi.lockdown").await?;
        let mut amfi = AmfiClient::new(Idevice::new(stream, LABEL));
        amfi.reveal_developer_mode_option_in_ui().await.map_err(|e| from_idevice(e, "revealing Developer Mode"))
    }

    /// The pairing state (`pairing.status`). A connector that does not answer it is asked for a lockdown value
    /// instead: an answer means paired, `device.notPaired`/`device.untrusted` mean not.
    pub async fn pairing_status(&self) -> Result<PairingStatus> {
        let udid = json!({ "udid": self.udid().as_str() });
        match self.conn().call(FacadeCall::new(facade::PAIRING_STATUS, udid.clone())).await {
            Ok(v) => serde_json::from_value(v)
                .map_err(|e| device_error(codes::device::PROTOCOL, format!("pairing.status: unexpected result: {e}"))),
            Err(e) if crate::io::unanswered(&e) => {
                let probe = json!({ "udid": self.udid().as_str(), "key": "ProductVersion" });
                match self.conn().call(FacadeCall::new(facade::LOCKDOWN_GET_VALUE, probe)).await {
                    Ok(_) => Ok(PairingStatus { record_found: true, paired: true, error: None }),
                    Err(e) if e.code() == codes::device::NOT_PAIRED || e.code() == codes::device::UNTRUSTED => {
                        Ok(PairingStatus {
                            record_found: e.code() == codes::device::UNTRUSTED,
                            paired: false,
                            error: Some(e.code().as_str().to_owned()),
                        })
                    }
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// `device.pair`: validates an existing record, else runs the Trust flow (device.md §6.2). Reports
    /// `trust-pending` to watchers while the dialog is up.
    pub async fn pair(&self, host_name: Option<&str>) -> Result<PairOutcome> {
        let _guard = self.actor.device_lock.lock().await;
        if self.pairing_status().await?.paired {
            return Ok(PairOutcome::Paired);
        }
        let call = self.conn().call(FacadeCall::new(
            facade::PAIRING_PAIR,
            json!({ "udid": self.udid().as_str(), "hostName": host_name, "timeoutMs": 120_000 }),
        ));
        tokio::pin!(call);
        let result = tokio::select! {
            r = &mut call => r,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                self.actor.update(|c| c.state = DeviceState::TrustPending);
                self.changed();
                call.await
            }
        }?;
        let outcome = match result.get("status").and_then(|s| s.as_str()) {
            Some("paired") => PairOutcome::Paired,
            Some("pending") => PairOutcome::Pending,
            Some("denied") => PairOutcome::Denied,
            Some("locked") => {
                self.set_locked(Tri::Yes);
                PairOutcome::Pending
            }
            other => {
                return Err(device_error(
                    codes::device::PROTOCOL,
                    format!("pairing.pair: unexpected status {other:?}"),
                ));
            }
        };
        self.actor.update(|c| {
            c.state = match outcome {
                PairOutcome::Paired => DeviceState::Ready,
                PairOutcome::Pending => DeviceState::TrustPending,
                PairOutcome::Denied => DeviceState::NeedsTrust,
            };
            if outcome == PairOutcome::Paired {
                // Re-read the full identity with the new record.
                c.identity = None;
            }
        });
        self.changed();
        Ok(outcome)
    }

    /// `device.prepare` (device.md §7): pairs if needed, reveals Developer Mode on request, mounts the DDI when
    /// missing (from `ddi_source`, else the `[device.ddi]` order), then reports fresh readiness.
    pub async fn prepare(&self, opts: PrepareOptions) -> Result<Readiness> {
        if !self.pairing_status().await?.paired {
            match self.pair(None).await? {
                PairOutcome::Paired => {}
                PairOutcome::Pending => {
                    return Err(device_error(codes::device::TRUST_PENDING, "the Trust dialog was not answered"));
                }
                PairOutcome::Denied => {
                    return Err(device_error(codes::device::UNTRUSTED, "the iPhone refused to trust this computer"));
                }
            }
        }
        self.ensure_identity().await?;
        let devmode = self.developer_mode().await.unwrap_or(Tri::Unknown);
        if devmode == Tri::No {
            if opts.reveal_developer_mode {
                self.reveal_developer_mode().await?;
            }
            return Err(device_error(codes::device::DEVELOPER_MODE_DISABLED, "Developer Mode is off on this iPhone"));
        }
        let progress = crate::NoProgress;
        match &opts.ddi_source {
            Some(src) => {
                self.ddi_mount(src, false, &progress).await?;
            }
            None => {
                self.ensure_ddi().await?;
            }
        }
        self.readiness(true).await
    }
}

/// The result of `device.pair`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairOutcome {
    Paired,
    Pending,
    Denied,
}

/// `device.prepare` options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrepareOptions {
    pub ddi_source: Option<DdiSource>,
    pub reveal_developer_mode: bool,
}

fn tunnel_state(r: &TunnelStatusRecord) -> TunnelState {
    match r.state.as_str() {
        "up" => TunnelState::Up {
            generation: r.generation.unwrap_or_default(),
            since: r
                .since
                .as_deref()
                .and_then(|s| time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
                .unwrap_or_else(time::OffsetDateTime::now_utc),
        },
        "failed" => TunnelState::Failed {
            error: r
                .error
                .clone()
                .unwrap_or_else(|| device_error(codes::device::TUNNEL_FAILED, "the tunnel failed").to_data()),
        },
        _ => TunnelState::Down,
    }
}

/// `device.readiness`: pairing, Developer Mode, DDI, lock hint and tunnel; cached until `refresh`.
pub async fn readiness(h: &DeviceHandle, refresh: bool) -> Result<Readiness> {
    if !refresh {
        let c = h.actor.cached();
        if let Some(mut r) = c.readiness {
            r.locked = c.locked;
            return Ok(r);
        }
    }
    let pairing = h.pairing_status().await?;
    let mut r = Readiness { paired: pairing.paired, locked: h.actor.cached().locked, ..Default::default() };
    if pairing.paired {
        if let Err(e) = h.ensure_identity().await {
            tracing::debug!(error = %e.message(), "identity read failed during readiness");
        }
        r.developer_mode = h.developer_mode().await.unwrap_or(Tri::Unknown);
        r.ddi = match h.ddi_status().await {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::debug!(error = %e.message(), "DDI status failed during readiness");
                None
            }
        };
    }
    if let Ok(v) = h.conn().call(FacadeCall::new(facade::TUNNEL_STATUS, json!({ "udid": h.udid().as_str() }))).await
        && let Ok(rec) = serde_json::from_value::<TunnelStatusRecord>(v)
    {
        r.tunnel = tunnel_state(&rec);
    }
    h.actor.update(|c| {
        c.readiness = Some(r.clone());
        if !pairing.paired && matches!(c.state, DeviceState::Ready | DeviceState::Attached) {
            c.state = DeviceState::NeedsTrust;
        }
    });
    h.changed();
    Ok(r)
}

/// `device.info`: the identity plus a few more lockdown values (no serials, no addresses).
pub async fn info(h: &DeviceHandle) -> Result<DeviceInfo> {
    h.ensure_identity().await?;
    let cached = h.actor.cached();
    let session = matches!(cached.state, DeviceState::Ready);
    let props = values(h, INFO_KEYS, None, session).await.unwrap_or_default();
    Ok(DeviceInfo {
        udid: h.udid().clone(),
        transport: h.actor.transport(),
        identity: cached.identity.unwrap_or_default(),
        state: cached.state,
        properties: props,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_values_map() {
        let v = json!({"DeviceName": "x", "ProductType": "iPhone13,2", "ProductVersion": "27.0", "BuildVersion": "24A437",
                       "ChipID": 33025, "CPUArchitecture": "arm64e"});
        let id = identity_from_values(v.as_object().expect("object"));
        assert_eq!(id.product_type.as_deref(), Some("iPhone13,2"));
        assert_eq!(id.chip_id, Some(0x8101));
        assert_eq!(id.ios_major(), Some(27));
    }

    #[test]
    fn tunnel_records_map_to_states() {
        let up = TunnelStatusRecord {
            state: "up".into(),
            generation: Some(3),
            since: Some("2026-10-11T09:00:00Z".into()),
            ..Default::default()
        };
        assert!(matches!(tunnel_state(&up), TunnelState::Up { generation: 3, .. }));
        let down = TunnelStatusRecord { state: "down".into(), ..Default::default() };
        assert_eq!(tunnel_state(&down), TunnelState::Down);
        let failed = TunnelStatusRecord { state: "failed".into(), ..Default::default() };
        assert!(matches!(tunnel_state(&failed), TunnelState::Failed { .. }));
    }
}
