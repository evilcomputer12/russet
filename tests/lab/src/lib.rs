// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet-lab`: the device-in-the-loop harness and suites (QA §3.9, L7). Lane A only: suites run through
//! `cargo xtask lab` against the live daemon of lane A's `RUSSET_HOME`, under the live-device lock (ARCHITECTURE
//! §3.3 item 7). The lab's own exit convention is 3 = skipped environment (ARCHITECTURE §13.4).
//!
//! F0 stub (W1); W9 fills it (the `smoke` suite).

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");
