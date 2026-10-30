// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `asc` handlers (§8.4). Owner: W12.
//!
//! `asc.get` is an allowlisted App Store Connect GET with a request-scoped token (SIGN §11): the client checks
//! the path against the allowlist on the URL as it will be sent and refuses encoded or relative segments. The
//! status and parsed JSON body are returned as is (errors included), so an agent can read raw resources without
//! a secret ever leaving the daemon.

use russet_account::AccountId;
use russet_core::Result;
use russet_ipc::Router;
use russet_protocol::asc as p;

use super::account::{accounts_of, redacted_cx};
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::AscGet, _, _>(|cx: DaemonCtx, _rq, params: p::AscGetParams| async move {
        redacted_cx(&cx, get(&cx, params).await).await
    });
}

/// `asc.get`.
pub async fn get(cx: &DaemonCtx, params: p::AscGetParams) -> Result<p::AscGetResult> {
    let svc = accounts_of(cx)?;
    let account = params.account_id.map(AccountId);
    let (status, body) = svc.asc_get(account.as_ref(), &params.path_and_query).await?;
    Ok(p::AscGetResult { status, body })
}
