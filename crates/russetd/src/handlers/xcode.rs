// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `xcode` handlers (PLAN §9.2). Owner: W28 (Wave 6).
//!
//! Pre-created by W1: the namespace has no methods yet, so [`register`] registers nothing. W28 registers each
//! method it adds to `russet_protocol::xcode`.

use russet_ipc::Router;

use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(_r: &mut Router<DaemonCtx>) {}
