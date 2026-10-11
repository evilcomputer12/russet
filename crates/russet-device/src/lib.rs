// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The device plane on idevice (ARCHITECTURE §5.3 `russet-device`; device.md §5): mux, pairing, the userspace
//! tunnel, RSD, the DDI, apps, launch, forwards, logs, screen, files, crashes, perf and symbols, behind the
//! [`io::ServiceConnector`] seam (testability contract T5).
//!
//! Identity is read lazily [D 63, D 83]: the daemon starts [`DeviceManager`] on the first device call, and a
//! daemon nobody asks never talks to the phone. F0 stub (W1); W4 fills it (Wave 1), W10 (Wave 2), W30 (Wi-Fi
//! and accessibility).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

use std::sync::Arc;

pub mod a11y;
pub mod actor;
pub mod apps;
pub mod crash;
pub mod ddi;
pub mod debug;
pub mod error;
pub mod extras;
pub mod fallback;
pub mod files;
pub mod io;
pub mod logs;
pub mod manager;
pub mod model;
pub mod mux;
pub mod pairing;
pub mod perf;
pub mod process;
pub mod screen;
pub mod symbols;
pub mod tunnel;
pub mod wifi;

pub use apps::{AppFilter, AppInfo, InstallOptions, InstalledApp};
pub use crash::{CrashQuery, IpsReport};
pub use debug::{BindSpec, ForwardHandle, ForwardTarget};
pub use logs::{LogFilter, LogStream};
pub use manager::{DeviceConfig, DeviceEvent, DeviceHandle, DeviceManager};
pub use model::{DeviceInfo, DeviceSelector, DeviceSummary, Readiness, Udid};
pub use perf::{SysmonOptions, SysmonStream};
pub use process::{LaunchRequest, LaunchedProcess, ProcessInfo};
pub use screen::{InputEvent, MirrorHandle, MirrorOptions, Screenshot};

/// Device doctor checks (DEV-25: mux, pairing, Developer Mode, DDI status, lock hint), for russetd's
/// doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}
