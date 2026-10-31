// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `xcode.*`: Xcode-project adaptation (`russet xcode adapt|dump-pif|show-build-settings`, ARCHITECTURE §8.7, K3). Owner: W28 (Wave 6).
//!
//! Pre-created by W1 with no methods (ARCHITECTURE §8 has no `xcode.*` row yet) so that W28 adds its methods here,
//! as protocol minor additions (§7.7), without editing the seam files. Declare each method with
//! `crate::rpc_method!` and list it in [`METHODS`].

use crate::{MethodInfo, NotificationInfo};

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
