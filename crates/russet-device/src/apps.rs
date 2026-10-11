// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Apps: install, uninstall, list (DEV-06). W4 defines the fields of these types (device.md §5.3, §6.4).

/// Install options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallOptions {}

/// What an install produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    pub bundle_id: String,
}

/// Which apps to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppFilter {}

/// One installed app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub bundle_id: String,
}
