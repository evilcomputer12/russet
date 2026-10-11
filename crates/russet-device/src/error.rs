// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The device error model and the NSError chain decoder (device.md §9). Device errors are `russet_core::Error` values with the device-family codes of ARCHITECTURE §13.3.
//!
//! F0 stub (W1): the module skeleton of device.md §5.2; the owning card fills it.
