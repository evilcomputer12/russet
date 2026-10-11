// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `doctor` handlers (§8.1). Owner: W3.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::doctor as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::DoctorRun, _, _>(not_implemented::<p::DoctorRun>);
}

/// The configuration, secrets and toolchain checks (CORE-12). russet-config, russet-secrets and russet-toolchain
/// are layer L2 and may not depend on russet-doctor (ARCHITECTURE §5.2), so W3 implements their checks here; the
/// doctor seam (`doctor_registry.rs`) registers them.
pub fn checks() -> Vec<std::sync::Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
