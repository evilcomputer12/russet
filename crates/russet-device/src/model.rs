// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Device identity and state (device.md §5.3). These are domain types; [`crate::wire`] converts them to the wire
//! shapes of ARCHITECTURE §9.5 (for example the flat `DeviceSummary.state`).

use std::fmt;

use russet_core::error::ErrorData;
use time::OffsetDateTime;

use crate::ddi::DdiStatus;

/// A device UDID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Udid(pub String);

impl Udid {
    /// The UDID as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A redacted form for logs and human output: the UDID is personal data (AGENTS.md §8).
    pub fn redacted(&self) -> String {
        redact_udid(&self.0)
    }
}

impl fmt::Display for Udid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Udid {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// `00008101-…` for logs: keeps the chip prefix, which is public, and drops the unique part.
pub fn redact_udid(udid: &str) -> String {
    match udid.split_once('-') {
        Some((chip, _)) if chip.len() == 8 => format!("{chip}-…"),
        _ => "<udid>".to_owned(),
    }
}

/// How a caller names a device: a UDID, an alias, or "the only ready device".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DeviceSelector {
    /// Exactly one ready device.
    #[default]
    Only,
    Udid(Udid),
    Alias(String),
}

impl DeviceSelector {
    /// The selector for an optional `udid` parameter (a UDID or an alias, ARCHITECTURE §7.3). Strings that look
    /// like UDIDs (hex, 24 or more characters, optionally with one dash) are UDIDs; anything else is an alias.
    pub fn parse(s: Option<&str>) -> Self {
        match s.map(str::trim).filter(|s| !s.is_empty()) {
            None => Self::Only,
            Some(s) if looks_like_udid(s) => Self::Udid(Udid(s.to_owned())),
            Some(s) => Self::Alias(s.to_owned()),
        }
    }
}

fn looks_like_udid(s: &str) -> bool {
    let hex = s.chars().filter(|c| *c != '-').count();
    hex >= 24 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-') && s.matches('-').count() <= 4
}

/// How a device is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    Usb,
    Wifi,
}

/// `yes`, `no` or `unknown`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Tri {
    Yes,
    No,
    #[default]
    Unknown,
}

impl From<bool> for Tri {
    fn from(b: bool) -> Self {
        if b { Tri::Yes } else { Tri::No }
    }
}

/// The device's code-signing monitor (ARCHITECTURE §9.5) [D 71].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeSigningMonitor {
    Txm,
    Ppl,
    None,
    Unknown,
}

/// The ChipID × iOS-major table of ARCHITECTURE §9.5 [D 71], generated from public BuildManifests (derived
/// booleans only): iOS 26 runs TXM on ChipID ≥ 0x8110; iOS 27 runs TXM on every supported chip except 0x8027
/// (the A12Z iPad Pro). Older releases and chips are PPL on A12 and later (ChipID ≥ 0x8020), else unknown.
pub fn code_signing_monitor(chip_id: Option<u32>, ios_major: Option<u32>) -> CodeSigningMonitor {
    let (Some(chip), Some(major)) = (chip_id, ios_major) else {
        return CodeSigningMonitor::Unknown;
    };
    match major {
        27.. if chip == 0x8027 => CodeSigningMonitor::Ppl,
        27.. if chip >= 0x8020 => CodeSigningMonitor::Txm,
        26 if chip >= 0x8110 => CodeSigningMonitor::Txm,
        17..=26 if chip >= 0x8020 => CodeSigningMonitor::Ppl,
        _ => CodeSigningMonitor::Unknown,
    }
}

/// The public marketing name of a product type (device.md §6.1: a small built-in table of public names; no Apple
/// files). Unknown types return `None`.
pub fn marketing_name(product_type: &str) -> Option<&'static str> {
    let name = match product_type {
        "iPhone11,2" => "iPhone XS",
        "iPhone11,4" | "iPhone11,6" => "iPhone XS Max",
        "iPhone11,8" => "iPhone XR",
        "iPhone12,1" => "iPhone 11",
        "iPhone12,3" => "iPhone 11 Pro",
        "iPhone12,5" => "iPhone 11 Pro Max",
        "iPhone12,8" => "iPhone SE (2nd generation)",
        "iPhone13,1" => "iPhone 12 mini",
        "iPhone13,2" => "iPhone 12",
        "iPhone13,3" => "iPhone 12 Pro",
        "iPhone13,4" => "iPhone 12 Pro Max",
        "iPhone14,2" => "iPhone 13 Pro",
        "iPhone14,3" => "iPhone 13 Pro Max",
        "iPhone14,4" => "iPhone 13 mini",
        "iPhone14,5" => "iPhone 13",
        "iPhone14,6" => "iPhone SE (3rd generation)",
        "iPhone14,7" => "iPhone 14",
        "iPhone14,8" => "iPhone 14 Plus",
        "iPhone15,2" => "iPhone 14 Pro",
        "iPhone15,3" => "iPhone 14 Pro Max",
        "iPhone15,4" => "iPhone 15",
        "iPhone15,5" => "iPhone 15 Plus",
        "iPhone16,1" => "iPhone 15 Pro",
        "iPhone16,2" => "iPhone 15 Pro Max",
        "iPhone17,1" => "iPhone 16 Pro",
        "iPhone17,2" => "iPhone 16 Pro Max",
        "iPhone17,3" => "iPhone 16",
        "iPhone17,4" => "iPhone 16 Plus",
        "iPhone17,5" => "iPhone 16e",
        _ => return None,
    };
    Some(name)
}

/// Lockdown identity values, read lazily on the first client request (ARCHITECTURE §5.3) [D 63].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    /// `DeviceName`: personal data; never logged.
    pub name: Option<String>,
    /// `ProductType`, for example `iPhone13,2`.
    pub product_type: Option<String>,
    /// `ProductVersion`, for example `27.0`.
    pub os_version: Option<String>,
    /// `BuildVersion`, for example `24A437`.
    pub build: Option<String>,
    pub device_class: Option<String>,
    pub hardware_model: Option<String>,
    pub cpu_architecture: Option<String>,
    /// `ChipID`, for example `0x8101`.
    pub chip_id: Option<u32>,
}

impl Identity {
    /// The iOS major version.
    pub fn ios_major(&self) -> Option<u32> {
        self.os_version.as_deref().and_then(|v| v.split('.').next()).and_then(|m| m.parse().ok())
    }

    /// The marketing name from the built-in table.
    pub fn marketing_name(&self) -> Option<&'static str> {
        self.product_type.as_deref().and_then(marketing_name)
    }

    /// The code-signing monitor from the ChipID × iOS-major table.
    pub fn code_signing_monitor(&self) -> CodeSigningMonitor {
        code_signing_monitor(self.chip_id, self.ios_major())
    }
}

/// The richer device state (device.md §5.3); the wire form is the flat `DeviceSummary.state` plus `error`.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum DeviceState {
    /// Seen by the mux, nothing checked yet.
    #[default]
    Attached,
    /// No valid pair record (`InvalidHostID`).
    NeedsTrust,
    /// The Trust dialog is up (`PairingDialogResponsePending`).
    TrustPending,
    /// Paired; [`Readiness`] tells what else is missing.
    Ready,
    Unavailable {
        error: ErrorData,
    },
}

/// The userspace tunnel's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum TunnelState {
    #[default]
    Down,
    Up {
        generation: u64,
        since: OffsetDateTime,
    },
    Failed {
        error: ErrorData,
    },
}

/// Pairing, Developer Mode, DDI, lock and tunnel state (`device.readiness`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Readiness {
    pub paired: bool,
    /// `unknown` before iOS 16 answers or when the session failed.
    pub developer_mode: Tri,
    /// The mounted DDI; `None` when its status could not be read.
    pub ddi: Option<DdiStatus>,
    /// iOS 27 cannot be asked (`getlockstate` is not implemented): `yes` only after an operation reported
    /// "Locked", `no` after the next successful launch, else `unknown` (device.md F9).
    pub locked: Tri,
    pub tunnel: TunnelState,
}

/// A device as `device.list` reports it (ARCHITECTURE §9.5 `DeviceSummary`, before conversion).
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceSummary {
    pub udid: Udid,
    pub transport: Transport,
    /// `None` until the identity was read (lazily).
    pub identity: Option<Identity>,
    pub state: DeviceState,
    /// `None` until a client asked for readiness.
    pub readiness: Option<Readiness>,
}

impl DeviceSummary {
    /// A device the mux just reported: only its mux record is known [D 63].
    pub fn attached(udid: Udid, transport: Transport) -> Self {
        Self { udid, transport, identity: None, state: DeviceState::Attached, readiness: None }
    }
}

/// Lockdown values and RSD properties (`device.info`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub udid: Udid,
    pub transport: Transport,
    pub identity: Identity,
    pub state: DeviceState,
    /// Selected lockdown values that are safe to show (no serials, no addresses).
    pub properties: serde_json::Map<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_tell_udids_from_aliases() {
        assert_eq!(DeviceSelector::parse(None), DeviceSelector::Only);
        assert_eq!(DeviceSelector::parse(Some("  ")), DeviceSelector::Only);
        assert_eq!(
            DeviceSelector::parse(Some("00008101-000A1B2C3D4E5F60")),
            DeviceSelector::Udid(Udid("00008101-000A1B2C3D4E5F60".into()))
        );
        assert_eq!(
            DeviceSelector::parse(Some("0123456789abcdef0123456789abcdef01234567")),
            DeviceSelector::Udid(Udid("0123456789abcdef0123456789abcdef01234567".into()))
        );
        assert_eq!(DeviceSelector::parse(Some("iphone12")), DeviceSelector::Alias("iphone12".into()));
    }

    #[test]
    fn udids_are_redacted_for_logs() {
        assert_eq!(redact_udid("00008101-000A1B2C3D4E5F60"), "00008101-…");
        assert_eq!(redact_udid("0123456789abcdef0123456789abcdef01234567"), "<udid>");
    }

    #[test]
    fn the_csm_table_matches_architecture_9_5() {
        use CodeSigningMonitor::{Ppl, Txm, Unknown};
        // The rig: iPhone 12 (A14, 0x8101) on iOS 27.0 runs TXM (verified from the 24A437 firmware).
        assert_eq!(code_signing_monitor(Some(0x8101), Some(27)), Txm);
        assert_eq!(code_signing_monitor(Some(0x8101), Some(26)), Ppl);
        assert_eq!(code_signing_monitor(Some(0x8110), Some(26)), Txm);
        assert_eq!(code_signing_monitor(Some(0x8027), Some(27)), Ppl);
        assert_eq!(code_signing_monitor(None, Some(27)), Unknown);
        let id = Identity {
            product_type: Some("iPhone13,2".into()),
            os_version: Some("27.0".into()),
            chip_id: Some(0x8101),
            ..Default::default()
        };
        assert_eq!(id.ios_major(), Some(27));
        assert_eq!(id.marketing_name(), Some("iPhone 12"));
        assert_eq!(id.code_signing_monitor(), Txm);
    }
}
