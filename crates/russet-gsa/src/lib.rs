// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! GrandSlam authentication, two-factor and the developer-services session of an Apple ID (SA-18; SIGN §7.2–§7.4).
//!
//! Russet signs the user in with its own client, not xtool's: [`GsaClient::login`] runs SRP-6a against
//! `gsa.apple.com` (trusting only Apple Root CA), the two-factor flows (trusted-device push and SMS) verify the
//! code, and [`GsaClient::xcode_token`] obtains the `com.apple.gs.xcode.auth` app token that developerservices2
//! accepts ([`ds2`]). The same path serves paid and free teams; paid personal accounts come first.
//!
//! Secrets stay in zeroized buffers with redacting `Debug`, are persisted only through `russet-secrets`
//! ([`StoredSession`]) and are never logged. [`fake`] (feature `fake`) is the GrandSlam fake of the mocked suites.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::Arc;

mod client;
mod crypto;
pub mod ds2;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
mod profile;
mod session;
mod srp;

pub use client::GsaClient;
pub use ds2::{AppGroup, Ds2Session, Qh65b2, Team, TeamTier};
pub use profile::ClientProfile;
pub use session::{
    GsSession, LoginState, PendingTwoFactor, SecondFactor, StoredSession, TrustedNumber, TwoFactorVia, XcodeToken,
};

/// Doctor checks of this crate, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
