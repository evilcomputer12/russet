// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `russet` binary. Multi-call dispatch (`russet` or `russetd` as `argv[0]`) lives in [`russet_cli::main_entry`].

fn main() -> std::process::ExitCode {
    russet_cli::main_entry()
}
