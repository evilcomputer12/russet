// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet-adi-host`: the sandboxed helper that loads Apple's runtime-downloaded Android ADI libraries; no
//! network; JSON-lines IPC (ARCHITECTURE §3.1; SIGN §7.5). Layer L3 binary: F crates and russet-core only.
//!
//! F0 stub (W1); W22 fills it (SA-19).

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("russet-adi-host: not implemented yet (W22)");
    ExitCode::from(1)
}
