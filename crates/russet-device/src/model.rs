// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Device identity and state (device.md §5.3). These are domain types; russet-protocol converts them to the
//! wire shapes of ARCHITECTURE §9.5 (for example the flat `DeviceSummary.state`).

/// A device UDID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Udid(pub String);

/// How a caller names a device: a UDID, an alias, or "the only ready device".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DeviceSelector {
    /// Exactly one ready device.
    #[default]
    Only,
    Udid(Udid),
    Alias(String),
}

/// A device as `device.list` reports it. W4 defines the fields (device.md §5.3, ARCHITECTURE §9.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSummary {
    pub udid: Udid,
}

/// Lockdown values and RSD properties (`device.info`). W4 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    _f0: (),
}

/// Pairing, Developer Mode, DDI, lock and tunnel state (`device.readiness`). W4 defines the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    _f0: (),
}
