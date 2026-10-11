// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `files` handlers (§8.2). Owner: W10.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::files as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::FilesList, _, _>(not_implemented::<p::FilesList>);
    r.request::<p::FilesPull, _, _>(not_implemented::<p::FilesPull>);
    r.request::<p::FilesPush, _, _>(not_implemented::<p::FilesPush>);
    r.request::<p::FilesRemove, _, _>(not_implemented::<p::FilesRemove>);
}
