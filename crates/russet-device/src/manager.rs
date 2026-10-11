// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`DeviceManager`] (one actor per UDID) and the cloneable [`DeviceHandle`] (ARCHITECTURE §5.3).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russet_core::Result;
use russet_core::progress::ProgressSink;
use tokio::sync::broadcast;

use crate::apps::{AppFilter, AppInfo, InstallOptions, InstalledApp};
use crate::crash::{CrashQuery, IpsReport};
use crate::debug::{BindSpec, ForwardHandle, ForwardTarget};
use crate::io::ServiceConnector;
use crate::logs::{LogFilter, LogStream};
use crate::model::{DeviceInfo, DeviceSelector, DeviceSummary, Readiness, Udid};
use crate::perf::{SysmonOptions, SysmonStream};
use crate::process::{LaunchRequest, LaunchedProcess, ProcessInfo};
use crate::screen::{InputEvent, MirrorHandle, MirrorOptions, Screenshot};

/// Device-plane configuration (mux endpoint, tunnel settings, pymobiledevice3 path, state directory, Wi-Fi).
/// W4 defines the remaining fields (device.md §5.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceConfig {
    /// Pair records (Wi-Fi), caches and mount records live here.
    pub state_dir: PathBuf,
}

/// A device-registry event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    Added(DeviceSummary),
    Changed(DeviceSummary),
    Removed(Udid),
}

/// The device registry: one actor per attached UDID.
#[derive(Debug)]
pub struct DeviceManager {
    _f0: (),
}

impl DeviceManager {
    pub async fn start(cfg: DeviceConfig, conn: Arc<dyn ServiceConnector>) -> Result<Arc<Self>> {
        todo!("W4 (DEV-02)")
    }
    pub fn devices(&self) -> Vec<DeviceSummary> {
        todo!("W4 (DEV-02)")
    }
    pub fn events(&self) -> broadcast::Receiver<DeviceEvent> {
        todo!("W4 (DEV-02)")
    }
    pub fn get(&self, sel: &DeviceSelector) -> Result<DeviceHandle> {
        todo!("W4 (DEV-02)")
    }
}

/// A cloneable handle; every call is a message to the device's actor.
#[derive(Debug, Clone)]
pub struct DeviceHandle {
    _f0: (),
}

impl DeviceHandle {
    pub async fn info(&self) -> Result<DeviceInfo> {
        todo!("W4 (DEV-05)")
    }
    pub async fn readiness(&self, refresh: bool) -> Result<Readiness> {
        todo!("W4 (DEV-05)")
    }
    /// Installs a `.app` or `.ipa`.
    pub async fn install(&self, pkg: &Path, o: InstallOptions, p: &dyn ProgressSink) -> Result<InstalledApp> {
        todo!("W4 (DEV-06)")
    }
    pub async fn apps(&self, f: AppFilter) -> Result<Vec<AppInfo>> {
        todo!("W4 (DEV-06)")
    }
    pub async fn uninstall(&self, bundle_id: &str) -> Result<()> {
        todo!("W4 (DEV-06)")
    }
    /// CoreDevice launch with the DVT fallback; keeps the launch channel open (ARCHITECTURE §17.1).
    pub async fn launch(&self, r: LaunchRequest) -> Result<LaunchedProcess> {
        todo!("W4 (DEV-07)")
    }
    pub async fn signal(&self, pid: u32, sig: i32) -> Result<()> {
        todo!("W4 (DEV-07)")
    }
    pub async fn processes(&self) -> Result<Vec<ProcessInfo>> {
        todo!("W4 (DEV-07)")
    }
    /// A raw stream inside the tunnel (broker, symbols).
    pub async fn connect_rsd(&self, service: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        todo!("W4 (DEV-08)")
    }
    pub async fn connect_lockdown(&self, service: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        todo!("W4 (DEV-08)")
    }
    pub async fn forward(&self, t: ForwardTarget, b: BindSpec) -> Result<ForwardHandle> {
        todo!("W4 (DEV-08)")
    }
    pub async fn logs(&self, f: LogFilter) -> Result<LogStream> {
        todo!("W4 (DEV-09)")
    }
    pub async fn screenshot(&self) -> Result<Screenshot> {
        todo!("W10")
    }
    pub async fn mirror(&self, o: MirrorOptions) -> Result<MirrorHandle> {
        todo!("W10")
    }
    pub async fn input(&self, e: InputEvent) -> Result<()> {
        todo!("W10")
    }
    pub async fn crash_pull(&self, q: CrashQuery, dest: &Path) -> Result<Vec<IpsReport>> {
        todo!("W10")
    }
    pub async fn sysmon(&self, o: SysmonOptions) -> Result<SysmonStream> {
        todo!("W10")
    }
}
