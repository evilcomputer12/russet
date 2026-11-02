// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The device plane on idevice (ARCHITECTURE §5.3 `russet-device`; device.md §5): mux, pairing, the userspace
//! tunnel, RSD, the DDI, apps, launch, forwards, logs, screen, files, crashes, perf and symbols, behind the
//! [`io::ServiceConnector`] seam (testability contract T5).
//!
//! Identity is read lazily [D 63, D 83]: the daemon starts [`DeviceManager`] on the first device call, and a
//! daemon nobody asks never talks to the phone. W4 fills the Wave-1 capabilities; W10 adds screen, mirror, input,
//! files, crashes, perf, imported DDIs and the pymobiledevice3 fallback (Wave 2); W30 Wi-Fi and accessibility.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::Arc;

pub mod a11y;
pub mod actor;
pub mod apps;
pub mod config;
pub mod coredevice;
pub mod crash;
pub mod ddi;
pub mod debug;
pub mod doctor;
pub mod error;
pub mod extras;
pub mod fallback;
pub mod files;
pub mod ids;
pub mod io;
pub mod live_lock;
pub mod logs;
pub mod manager;
pub mod model;
pub mod mux;
pub mod pairing;
pub mod perf;
pub mod process;
pub mod rpc;
pub mod screen;
pub mod serve;
pub mod symbols;
pub mod tunnel;
pub mod wifi;
pub mod wire;

pub use apps::{AppFilter, AppInfo, InstallOptions, InstalledApp};
pub use crash::{CrashQuery, IpsReport};
pub use debug::{AcceptPolicy, BindSpec, ForwardHandle, ForwardOptions, ForwardTarget, LocalEndpoint};
pub use logs::{LogFilter, LogStream};
pub use manager::{DdiConfig, DeviceConfig, DeviceEvent, DeviceHandle, DeviceManager};
pub use model::{DeviceInfo, DeviceSelector, DeviceSummary, Readiness, Udid};
pub use perf::{SysmonOptions, SysmonStream};
pub use process::{
    ExitInfo, ExitReporter, ExitWatcher, LaunchRequest, LaunchVia, LaunchedProcess, ProcessInfo, exit_channel,
};
pub use screen::{
    AccessUnit, ButtonAction, HardwareButton, HevcConfig, InputEvent, MirrorHandle, MirrorOptions, Screenshot,
    TouchPhase, hevc_config,
};

/// A plaintext device stream, as [`io::ServiceConnector`] returns it.
pub type ReadWriteBox = Box<dyn idevice::ReadWrite>;

/// Device doctor checks (DEV-25: mux, pairing, Developer Mode, DDI status, lock hint), for russetd's
/// doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    doctor::checks()
}

/// A [`russet_core::progress::ProgressSink`] that drops everything (internal steps that report nowhere).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProgress;

impl russet_core::progress::ProgressSink for NoProgress {
    fn begin(&self, _title: &str, _cancellable: bool) {}
    fn report(&self, _msg: Option<&str>, _pct: Option<u8>) {}
    fn end(&self, _msg: Option<&str>) {}
    fn log(&self, _stream: russet_core::progress::LogStream, _line: &str) {}
}
