// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The run pipeline (`run.rs`): build → provision plan/apply → sign → install-if-changed (the install ledger) →
//! launch or debug (ARCHITECTURE §17.1).
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W7 fills and owns it.
