// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet-tool`: multi-call Xcode-tool shims for Swift Build and flutter_tools (ARCHITECTURE §3.1): `actool`,
//! `ibtool`, `xcstringstool`, `xcrun`, `codesign` (no-op), the `ld64.lld` trampoline and error shims for
//! closed tools. Layer F binary: F crates only.
//!
//! F0 stub (W1); W16 owns the binary, W21 and W21b their tool modules.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::process::ExitCode;

fn main() -> ExitCode {
    let tool = std::env::args().next().unwrap_or_default();
    eprintln!("russet-tool: `{tool}` is not implemented yet (W16)");
    ExitCode::from(1)
}
