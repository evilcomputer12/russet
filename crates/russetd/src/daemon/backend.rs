// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The device backend (ARCHITECTURE §3.3 item 7, §14.9 `RUSSET_DEVICE_BACKEND`) and the live-backend policy of
//! development builds [D 63, D 83].

use std::fmt;
use std::path::PathBuf;

use russet_core::error::{Error, Result};

/// `live` (default), `scripted:<scenario.toml>`, `replay:<trace>`, `mock:<addr>` or `none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceBackend {
    /// The real phone through usbmuxd (Linux) or AMDS (Windows), under the host-wide live-device lock.
    Live,
    /// A scripted `ServiceConnector` (russet-testkit, W8).
    Scripted(PathBuf),
    /// A recorded RDT trace (russet-testkit, W15).
    Replay(PathBuf),
    /// The mock device behind a mock usbmuxd at this address (QA T5, W15).
    Mock(String),
    /// No device plane at all: lane D until W8's scripted scenario exists.
    None,
}

impl DeviceBackend {
    /// Parses the value of `RUSSET_DEVICE_BACKEND`.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        let bad = || {
            Error::invalid_argument(format!("`{s}` is not a device backend")).with_hint(
                "RUSSET_DEVICE_BACKEND is live, scripted:<scenario.toml>, replay:<trace>, mock:<addr> or none.",
            )
        };
        Ok(match s {
            "" | "live" => Self::Live,
            "none" => Self::None,
            _ => match s.split_once(':') {
                Some(("scripted", p)) if !p.is_empty() => Self::Scripted(PathBuf::from(p)),
                Some(("replay", p)) if !p.is_empty() => Self::Replay(PathBuf::from(p)),
                Some(("mock", a)) if !a.is_empty() => Self::Mock(a.to_owned()),
                _ => return Err(bad()),
            },
        })
    }

    /// The backend `RUSSET_DEVICE_BACKEND` asks for (`live` when unset).
    pub fn from_env() -> Result<Self> {
        Self::parse(&std::env::var("RUSSET_DEVICE_BACKEND").unwrap_or_default())
    }

    /// Whether this is the real phone.
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }
}

impl fmt::Display for DeviceBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Live => f.write_str("live"),
            Self::Scripted(p) => write!(f, "scripted:{}", p.display()),
            Self::Replay(p) => write!(f, "replay:{}", p.display()),
            Self::Mock(a) => write!(f, "mock:{a}"),
            Self::None => f.write_str("none"),
        }
    }
}

/// Whether a daemon may use the `live` backend: release builds always; development builds only with
/// `RUSSET_LANE=A` or `russet daemon run --allow-live-device` (ARCHITECTURE §3.3 item 7).
pub fn live_allowed(development: bool, lane_a: bool, allow_live_device: bool) -> bool {
    !development || lane_a || allow_live_device
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backends_parse_and_print() {
        for (text, backend) in [
            ("live", DeviceBackend::Live),
            ("", DeviceBackend::Live),
            ("none", DeviceBackend::None),
            ("scripted:/w/default.toml", DeviceBackend::Scripted("/w/default.toml".into())),
            ("replay:/t.rdt", DeviceBackend::Replay("/t.rdt".into())),
            ("mock:127.0.0.1:27015", DeviceBackend::Mock("127.0.0.1:27015".into())),
        ] {
            let parsed = DeviceBackend::parse(text).expect(text);
            assert_eq!(parsed, backend);
            if !text.is_empty() {
                assert_eq!(parsed.to_string(), text);
            }
        }
        assert!(DeviceBackend::parse("scripted:").is_err());
        assert!(DeviceBackend::parse("usb").is_err());
    }

    #[test]
    fn development_builds_need_lane_a_or_the_flag() {
        assert!(live_allowed(false, false, false), "release builds");
        assert!(!live_allowed(true, false, false), "lane D development daemons refuse live");
        assert!(live_allowed(true, true, false));
        assert!(live_allowed(true, false, true));
    }
}
