// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Renderers (core §5.9, §12): the human reporter (anstream and indicatif) and the NDJSON reporter (`--json`).
//! Only this module prints to stdout.
//!
//! F0 stub (W1); W2 owns it (CORE-07).

use russet_core::Error;
use russet_ipc::ProgressValue;

/// Where a command's output goes.
pub trait Reporter: Send + Sync {
    fn progress(&self, p: &ProgressValue);
    fn result(&self, v: &serde_json::Value);
    fn error(&self, e: &Error);
}

/// Human output.
#[derive(Debug, Default)]
pub struct HumanReporter {
    _f0: (),
}

impl Reporter for HumanReporter {
    fn progress(&self, p: &ProgressValue) {
        todo!("W2 (CORE-07)")
    }
    fn result(&self, v: &serde_json::Value) {
        todo!("W2 (CORE-07)")
    }
    fn error(&self, e: &Error) {
        todo!("W2 (CORE-07)")
    }
}

/// `--json`: one JSON document, or NDJSON for streams.
#[derive(Debug, Default)]
pub struct JsonReporter {
    _f0: (),
}

impl Reporter for JsonReporter {
    fn progress(&self, p: &ProgressValue) {
        todo!("W2 (CORE-07)")
    }
    fn result(&self, v: &serde_json::Value) {
        todo!("W2 (CORE-07)")
    }
    fn error(&self, e: &Error) {
        todo!("W2 (CORE-07)")
    }
}
