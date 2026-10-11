// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Test doubles for every testability contract (QA §2): scripted, replay and recording `ServiceConnector`s,
//! a mock usbmuxd, cassettes, HTTP fakes, the temp-home daemon fixture (`TempHome`) and RDT traces.
//!
//! Tests of an L3 crate that use the testkit are integration tests under that crate's `tests/`, never
//! `#[cfg(test)]` unit modules: the testkit ↔ crate dev-dependency cycle would otherwise compile two copies of
//! the crate's types (ARCHITECTURE §5.1).
//!
//! F0 stub (W1); W8 owns the crate, W7 owns [`debug`].

/// Debug-plane test doubles: the fake debugserver, the pre-attach relay and the JIT demo harness, ported from
/// `docs/design/run-debug-prototypes` (RD-02). Pre-created for W7.
pub mod debug;
