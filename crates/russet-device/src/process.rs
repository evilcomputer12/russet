// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Launch, signals, process list, console and exit detection (DEV-07). W4 defines the remaining fields
//! (device.md §5.3, §6.5).

use std::collections::BTreeMap;

/// A launch request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchRequest {
    pub bundle_id: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub suspended: bool,
    pub kill_existing: bool,
    /// Attach CoreDevice stdio (`openstdiosocket`).
    pub console: bool,
}

/// A launched process; the launch channel stays open until the debugger attaches (ARCHITECTURE §17.1).
#[derive(Debug)]
pub struct LaunchedProcess {
    pub pid: u32,
    /// The device path, from CoreDevice's `executableURL`.
    pub executable: String,
}

/// One process on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
}
