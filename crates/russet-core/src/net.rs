// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! HTTP endpoints (testability contract T3). Overridden by `RUSSET_ENDPOINTS_FILE`; `RUSSET_OFFLINE=1`
//! refuses non-loopback hosts.

use url::Url;

/// Base URLs of the external services Russet talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub asc: Url,
    pub gsa: Url,
    pub tss: Url,
    pub anisette_libs: Url,
    pub github: Url,
    pub flutter_storage: Url,
}
