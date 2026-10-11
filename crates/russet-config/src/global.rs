// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The global `config.toml` (ARCHITECTURE §14.3), in the config directory. Accounts are not here (`accounts.toml`,
//! [D 14]) and neither is the MCP policy (`mcp-policy.toml`, [D 44]).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::value::{ByteSize, DurationStr};

/// The global configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[schemars(title = "config.toml", description = "Russet's global configuration (ARCHITECTURE §14.3).")]
pub struct GlobalConfig {
    /// The file-format version; only `1` exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    pub daemon: DaemonSection,
    pub device: DeviceSection,
    pub storage: StorageSection,
    pub devices: DevicesSection,
    pub web: WebSection,
    pub mcp: McpSection,
    pub fallback: FallbackSection,
    pub update: UpdateSection,
    pub privacy: PrivacySection,
}

/// `[daemon]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct DaemonSection {
    /// `"0"` never idles out.
    pub idle_timeout: DurationStr,
    #[schemars(range(min = 1))]
    pub max_parallel_jobs: u32,
    #[schemars(range(min = 1))]
    pub max_build_jobs: u32,
    #[schemars(range(min = -20, max = 19))]
    pub build_nice: i32,
    /// A tracing filter, for example `info` or `info,russetd=debug`.
    pub log_filter: String,
    pub log_retention_days: u32,
    /// WSL split mode only, for example `127.0.0.1:0`; empty = off.
    pub listen_tcp: String,
}

impl Default for DaemonSection {
    fn default() -> Self {
        Self {
            idle_timeout: DurationStr::mins(30),
            max_parallel_jobs: 4,
            max_build_jobs: 1,
            build_nice: 10,
            log_filter: "info".into(),
            log_retention_days: 7,
            listen_tcp: String::new(),
        }
    }
}

/// `[device]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct DeviceSection {
    pub keep_warm: DurationStr,
    pub lease_grace: DurationStr,
    /// `auto`, `usbmuxd:<path>`, `amds` or `netmuxd:<addr>`.
    pub mux: String,
    pub ddi: DdiSection,
}

impl Default for DeviceSection {
    fn default() -> Self {
        Self {
            keep_warm: DurationStr::mins(10),
            lease_grace: DurationStr::secs(2),
            mux: "auto".into(),
            ddi: DdiSection::default(),
        }
    }
}

/// `[device.ddi]` [D 62, D 82]: `ensure_ddi` tries `order` and stops at the first source that mounts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct DdiSection {
    pub order: Vec<DdiSourceKind>,
    /// A DDI root holding `Restore/` and `version.plist` (a copy of a Mac's
    /// `/Library/Developer/DeveloperDiskImages/iOS_DDI`), or a directory that holds `BuildManifest.plist`.
    pub dir: String,
    /// `pmd3-cache` and `pmd3-auto-mount` are mirror content; allowed only after the consent text.
    pub allow_mirror: bool,
    /// Cache the Cryptex1 IM4M per (ECID, ddiBuild, nonce), mode 0600, for offline remounts.
    pub ticket_cache: bool,
}

impl Default for DdiSection {
    fn default() -> Self {
        Self {
            order: vec![DdiSourceKind::Dir, DdiSourceKind::Imported],
            dir: String::new(),
            allow_mirror: false,
            ticket_cache: false,
        }
    }
}

/// `[device.ddi] order` entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DdiSourceKind {
    Dir,
    Imported,
    Pmd3Cache,
    Pmd3AutoMount,
}

/// `[storage]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct StorageSection {
    /// Checked before every download, import, symbol fetch or cache growth (ARCHITECTURE §6.7).
    pub min_free_space: ByteSize,
    pub symbol_quota: ByteSize,
}

impl Default for StorageSection {
    fn default() -> Self {
        Self { min_free_space: ByteSize::gib(5), symbol_quota: ByteSize::gib(20) }
    }
}

/// `[devices]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct DevicesSection {
    /// Alias → UDID.
    pub aliases: BTreeMap<String, String>,
}

/// `[web]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct WebSection {
    pub listen: String,
}

impl Default for WebSection {
    fn default() -> Self {
        Self { listen: "127.0.0.1:0".into() }
    }
}

/// `[mcp]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct McpSection {
    pub http: McpHttpSection,
}

/// `[mcp.http]`: the opt-in MCP HTTP endpoint (ARCHITECTURE §6.6, §11.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct McpHttpSection {
    pub enabled: bool,
    pub listen: String,
    pub expose_docker: bool,
}

impl Default for McpHttpSection {
    fn default() -> Self {
        Self { enabled: false, listen: "127.0.0.1:43117".into(), expose_docker: false }
    }
}

/// `[fallback]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct FallbackSection {
    /// `auto`, `off` or the path of a `pymobiledevice3` executable.
    pub pymobiledevice3: String,
}

impl Default for FallbackSection {
    fn default() -> Self {
        Self { pymobiledevice3: "auto".into() }
    }
}

/// `[update]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct UpdateSection {
    pub check: UpdateCheck,
    pub channel: UpdateChannel,
}

impl Default for UpdateSection {
    fn default() -> Self {
        Self { check: UpdateCheck::Notify, channel: UpdateChannel::Stable }
    }
}

/// `[update] check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateCheck {
    Notify,
    Off,
}

/// `[update] channel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateChannel {
    Stable,
    Beta,
}

/// `[privacy]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct PrivacySection {
    /// Russet has no telemetry; the key only documents that.
    pub telemetry: Telemetry,
}

impl Default for PrivacySection {
    fn default() -> Self {
        Self { telemetry: Telemetry::Off }
    }
}

/// `[privacy] telemetry`: always `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Telemetry {
    Off,
}
