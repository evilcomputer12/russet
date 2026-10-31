// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! DVT sysmontap, energy and network samples (W10). W10 defines the fields.

/// Sampling options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SysmonOptions {}

/// A live sample stream.
#[derive(Debug)]
pub struct SysmonStream {
    _f0: (),
}
