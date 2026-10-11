// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Crash report pull and the `.ips` parser (W10, W11). W10 defines the fields.

/// Which crash reports to pull.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrashQuery {}

/// A parsed `.ips` report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpsReport {
    _f0: (),
}
