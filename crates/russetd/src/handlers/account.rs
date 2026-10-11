// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `account` handlers (§8.4). Owner: W6 (Wave 1), W12 (Wave 2), W22 (appleId and anisette rows, Wave 3).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::account as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::AccountList, _, _>(not_implemented::<p::AccountList>);
    r.request::<p::AccountAddAscKey, _, _>(not_implemented::<p::AccountAddAscKey>);
    r.request::<p::AccountImportXtool, _, _>(not_implemented::<p::AccountImportXtool>);
    r.request::<p::AccountVerify, _, _>(not_implemented::<p::AccountVerify>);
    r.request::<p::AccountRemove, _, _>(not_implemented::<p::AccountRemove>);
    r.request::<p::AccountSetDefault, _, _>(not_implemented::<p::AccountSetDefault>);
    r.request::<p::AccountAppleIdBegin, _, _>(not_implemented::<p::AccountAppleIdBegin>);
    r.request::<p::AccountAppleIdRequestCode, _, _>(not_implemented::<p::AccountAppleIdRequestCode>);
    r.request::<p::AccountAppleIdSubmitCode, _, _>(not_implemented::<p::AccountAppleIdSubmitCode>);
    r.request::<p::AccountAnisetteStatus, _, _>(not_implemented::<p::AccountAnisetteStatus>);
    r.request::<p::AccountAnisetteFetchLibraries, _, _>(not_implemented::<p::AccountAnisetteFetchLibraries>);
}
