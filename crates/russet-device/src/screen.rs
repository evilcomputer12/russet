// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Screenshots, the live mirror and HID input (W10; device.md §6.8). W10 defines the fields.

/// A screenshot (PNG).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screenshot {
    pub png: Vec<u8>,
}

/// Mirror options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirrorOptions {}

/// A running mirror session.
#[derive(Debug)]
pub struct MirrorHandle {
    _f0: (),
}

/// One HID input event.
#[derive(Debug, Clone, PartialEq)]
pub struct InputEvent {
    _f0: (),
}
