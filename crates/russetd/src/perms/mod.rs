// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The permission engine: classes per connection kind, MCP tiers and the confirmation and approval rules
//! (ARCHITECTURE §7.5, §11.3) [D 6, D 66].
//!
//! F0 stub (W1); W2 owns it.

/// Decides per connection and per call (`DaemonCtx.perms`).
#[derive(Debug, Default)]
pub struct PermissionEngine {
    _f0: (),
}
