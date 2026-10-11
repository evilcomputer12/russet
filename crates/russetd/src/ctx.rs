// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`DaemonCtx`]: what every handler gets (ARCHITECTURE §5.3) [D 86]. Every field is cheap to clone.

use std::sync::Arc;

use russet_core::Result;
use russet_core::dirs::RussetDirs;
use tokio_util::sync::CancellationToken;

use crate::approvals::ApprovalService;
use crate::audit::AuditLog;
use crate::jobs::JobManager;
use crate::leases::LeaseTable;
use crate::perms::PermissionEngine;
use crate::subs::SubscriptionHub;
use crate::web::WebEndpoint;

/// The daemon context handed to every handler.
#[derive(Clone)]
pub struct DaemonCtx {
    pub dirs: RussetDirs,
    pub config: Arc<russet_config::ConfigService>,
    pub secrets: Arc<dyn russet_secrets::SecretStore>,
    pub toolchains: Arc<russet_toolchain::Toolchains>,
    pub jobs: JobManager,
    pub leases: LeaseTable,
    pub subs: SubscriptionHub,
    pub perms: Arc<PermissionEngine>,
    pub approvals: ApprovalService,
    pub audit: AuditLog,
    pub web: WebEndpoint,
    pub devices: DeviceSlot,
    pub debug: Arc<russet_debug::DebugService>,
    pub accounts: Arc<russet_account::AccountService>,
    pub shutdown: CancellationToken,
}

/// The lazily started device plane: the backend, the live-device lock state and a
/// `OnceCell<Arc<DeviceManager>>` [D 83]. W2 owns it.
#[derive(Debug, Clone)]
pub struct DeviceSlot {
    _f0: Arc<()>,
}

/// russet-debug reaches the device plane through the slot, which starts the manager on first use [D 83].
impl russet_debug::DeviceServices for DeviceSlot {}

impl DeviceSlot {
    /// Starts `DeviceManager` (and so its connector) on the first call that passes the backend and live-device
    /// lock checks (ARCHITECTURE §3.3 item 7); otherwise `device.liveBackendRefused` (3019, with
    /// `details.holder` when another daemon holds the lock).
    pub async fn manager(&self) -> Result<Arc<russet_device::DeviceManager>> {
        todo!("W2 (development isolation guards)")
    }
}
