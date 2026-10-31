// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Anisette providers (local ADI through russet-adi-host, RemoteV3) and runtime acquisition of Apple's Android
//! libraries, with consent and hash pins; never redistributed (SA-19; SIGN §7.5).
//!
//! - [`AnisetteProvider`] is what GrandSlam (`russet-gsa`) and developerservices2 ask for the per-request machine
//!   headers (`X-Apple-I-MD`, `-MD-M`, `-MD-RINFO`, `-MD-LU`, `X-Mme-Device-Id`, …).
//! - [`LocalAdi`] runs Apple's ADI libraries in the sandboxed `russet-adi-host` child (no network: this crate does
//!   every HTTP request, the provisioning ones included) and provisions the machine against GrandSlam's
//!   `midStartProvisioning`/`midFinishProvisioning`.
//! - [`RemoteV3`] asks a self-hosted anisette-v3 server (opt-in; its operator learns this machine's ADI identity).
//! - [`libs`] fetches only the two libraries out of the Apple Music APK by HTTP range (about 2 MB of a 142 MB APK),
//!   checks them against `anisette-pins.json` and stores them 0700 under the data directory. They are never part
//!   of any Russet artefact, crash report or support bundle.
//! - [`apple_tls`] builds the HTTP client that trusts only Apple Root CA (SHA-256 pinned) for `gsa.apple.com`.
//!
//! Nothing here logs header values, ADI state or the local user UUID.
//!
//! W27c owns `windows`.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::Arc;

use async_trait::async_trait;
use russet_core::Result;

pub mod apple_tls;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
mod http;
pub mod libs;
pub mod local;
pub mod provisioning;
pub mod remote;
pub mod state;

/// The Windows ADI host shims (pre-created for W27c).
#[cfg(windows)]
pub mod windows;

pub use libs::{Abi, LibrarySet};
pub use local::{AdiBackend, LocalAdi, ProcessBackend};
pub use provisioning::{ClientIdentity, LookupUrls};
pub use remote::RemoteV3;
pub use state::AnisetteState;

/// The ADI DSID every provider provisions and asks OTPs for (SIGN §7.5).
pub const ADI_DSID: i64 = -2;

/// `X-Apple-I-MD-RINFO` when provisioning reports none (the value every known client sends).
pub const DEFAULT_RINFO: &str = "17106176";

/// Which provider answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnisetteKind {
    /// Apple's ADI libraries in `russet-adi-host`.
    Local,
    /// An anisette-v3 server.
    Remote { url: url::Url },
}

/// The machine headers of one request (SIGN §7.5). `Debug` prints the header names only.
#[derive(Clone, PartialEq, Eq)]
pub struct AnisetteHeaders {
    /// `X-Apple-I-MD`: the one-time password (base64).
    pub md: String,
    /// `X-Apple-I-MD-M`: the machine ID (base64).
    pub md_m: String,
    /// `X-Apple-I-MD-RINFO`: the routing info from provisioning.
    pub md_rinfo: String,
    /// `X-Apple-I-MD-LU`: `hex_upper(SHA256(local user UUID))`.
    pub md_lu: String,
    /// `X-Mme-Device-Id`: the local user UUID, uppercase.
    pub device_id: String,
    /// `X-Apple-I-SRL-NO` (`"0"`).
    pub serial: String,
    /// `X-Apple-I-Client-Time` (RFC 3339, seconds, UTC).
    pub client_time: String,
    /// `X-Apple-Locale` (`en_US`).
    pub locale: String,
    /// `X-Apple-I-TimeZone` (`UTC`).
    pub time_zone: String,
}

impl std::fmt::Debug for AnisetteHeaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnisetteHeaders").finish_non_exhaustive()
    }
}

impl AnisetteHeaders {
    /// The HTTP header names and values, in a stable order.
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("X-Apple-I-MD", self.md.clone()),
            ("X-Apple-I-MD-M", self.md_m.clone()),
            ("X-Apple-I-MD-RINFO", self.md_rinfo.clone()),
            ("X-Apple-I-MD-LU", self.md_lu.clone()),
            ("X-Mme-Device-Id", self.device_id.clone()),
            ("X-Apple-I-SRL-NO", self.serial.clone()),
            ("X-Apple-I-Client-Time", self.client_time.clone()),
            ("X-Apple-Locale", self.locale.clone()),
            ("X-Apple-I-TimeZone", self.time_zone.clone()),
        ]
    }
}

/// The current time as `X-Apple-I-Client-Time` wants it (`2026-10-31T12:00:00Z`).
pub fn client_time_now() -> String {
    let now = time::OffsetDateTime::now_utc().replace_nanosecond(0).unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    now.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

/// A source of anisette headers (SIGN §7.5).
#[async_trait]
pub trait AnisetteProvider: Send + Sync {
    fn kind(&self) -> AnisetteKind;
    /// Fresh headers for one request (provisions the machine first when needed).
    async fn headers(&self) -> Result<AnisetteHeaders>;
    /// The `X-Mme-Client-Info` these headers belong with.
    async fn client_info(&self) -> Result<String>;
    /// Erases the provisioning (ADI DSID -2) so the next request provisions again.
    async fn reset(&self) -> Result<()>;
}

/// A shared provider.
pub type SharedProvider = Arc<dyn AnisetteProvider>;

/// Whether local ADI runs on this host: Apple's Android libraries exist for x86_64 and arm64, and the loader and
/// sandbox of `russet-adi-host` are Linux-only until W27c (Windows reaches them through the engine's WSL
/// backend).
pub fn local_supported() -> bool {
    cfg!(target_os = "linux") && Abi::host().is_some()
}

/// Anisette doctor checks, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_header_values() {
        let h = AnisetteHeaders {
            md: "canary-otp".into(),
            md_m: "canary-mid".into(),
            md_rinfo: DEFAULT_RINFO.into(),
            md_lu: "canary-lu".into(),
            device_id: "canary-device".into(),
            serial: "0".into(),
            client_time: client_time_now(),
            locale: "en_US".into(),
            time_zone: "UTC".into(),
        };
        let text = format!("{h:?}");
        assert!(!text.contains("canary"), "{text}");
        assert_eq!(h.pairs()[0].0, "X-Apple-I-MD");
    }

    #[test]
    fn client_time_is_rfc3339_seconds() {
        let t = client_time_now();
        assert!(t.ends_with('Z') && t.len() == 20, "{t}");
    }
}
