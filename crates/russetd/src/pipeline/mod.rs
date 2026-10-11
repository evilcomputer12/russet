// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The run pipeline (ARCHITECTURE §17.1): resolve → build → provision plan/apply → sign to
//! `.russet/build/<profile>-iphoneos/signed/` → install-if-changed (the install ledger) → launch or debug.
//!
//! - the pipeline over four ports and russet-debug's `DeviceServices`, so it is tested with fakes: [`run`];
//! - the install ledger, `state/devices/<udid>/installs.json`: [`ledger`];
//! - the ports over the daemon's real services: [`live`];
//! - `run.subscribe`'s notification forwarder: [`subscribe`].
//!
//! [`lldb`] re-exports russet-debug's LLDB plans for the CLI, which may not depend on an L3 crate directly
//! (ARCHITECTURE §5.2) but reaches it through russetd.
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W7 owns it.

pub mod ledger;
pub mod live;
pub mod run;
pub mod subscribe;

pub use russet_debug::lldb;
