// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The async App Store Connect client (ARCHITECTURE §5.3; SIGN §5–§6): ES256 JWT, scoped GET tokens, a rate
//! limiter, open enums and a pluggable [`Auth`] (JWT, DS2).
//!
//! F0 stub (W1); W6 fills it (SA-02, GET-only in Wave 1); W22 owns [`ds2`].

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

use std::sync::Arc;
use std::time::Duration;

use russet_core::Result;
use secrecy::SecretString;

pub mod ds2;

/// How requests are authenticated (team-key JWT, or the free Apple ID's DS2 session).
pub trait Auth: Send + Sync {}

/// One entry of a scoped token, for example `GET /v1/apps`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeEntry(pub String);

/// Mints bearer tokens.
#[derive(Debug)]
pub struct TokenFactory {
    _f0: (),
}

impl TokenFactory {
    /// A cached bearer token (at most 19 minutes old).
    pub fn bearer(&self) -> Result<SecretString> {
        todo!("W6 (SA-02)")
    }
    /// A GET-scoped token for agents.
    pub fn scoped(&self, scope: &[ScopeEntry], lifetime: Duration) -> Result<SecretString> {
        todo!("W6 (SA-02)")
    }
}

/// The typed client: certificates, devices, bundle IDs, capabilities, profiles, apps, builds, build uploads,
/// TestFlight and CI resources, plus an allowlisted `raw_get`. W6 adds the resource methods.
#[derive(Debug)]
pub struct AscClient {
    _f0: (),
}

/// App Store Connect doctor checks, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
