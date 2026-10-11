// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `russet` CLI (ARCHITECTURE §5.3 `russet-cli`, §8.7; core §5.9): multi-call dispatch, the clap tree,
//! renderers and `--json`.
//!
//! F0 stub (W1); W2 owns this file (CORE-07). Until W2 lands the daemon connection, every command reports
//! `core.notImplemented` (exit 1); `--help` already shows the whole tree.

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

use std::process::ExitCode;

use clap::Command;
use russet_core::error::{Error, codes};
use russet_ipc::Peer;

pub mod cmd;
pub mod global;
pub mod render;
pub mod spawn;

/// What every command gets: the daemon peer, the reporter and the global flags.
pub struct CliCtx {
    pub peer: Peer,
    pub reporter: Box<dyn render::Reporter>,
    pub global: global::GlobalArgs,
}

/// The whole command tree: global flags plus every area's commands (ARCHITECTURE §8.7).
pub fn root_command() -> Command {
    let root = Command::new("russet")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Build, run, debug and ship iOS apps from Linux and Windows, with a real iPhone and no Mac")
        .subcommand_required(true)
        .arg_required_else_help(true);
    global::args(root).subcommands(cmd::commands())
}

/// The process entry point: parses the command line and dispatches.
pub fn main_entry() -> ExitCode {
    // One rustls provider for the whole binary: ring (ARCHITECTURE §16.6 [D 21]).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let matches = root_command().get_matches();
    // F0 stub: W2 adds the runtime, the daemon connection (`CliCtx`) and `cmd::run`.
    let name = matches.subcommand_name().unwrap_or("russet");
    let err = Error::new(codes::core::NOT_IMPLEMENTED, format!("`russet {name}` is not implemented yet"))
        .with_hint("This command lands in a later workstream; see docs/PLAN.md.");
    let code = err.code();
    eprintln!("{} ({}): {}. Hint: {}", code.as_str(), code.number(), err.message(), err.hint().unwrap_or_default());
    ExitCode::from(code.exit_code())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Every top-level command of ARCHITECTURE §8.7.
    const TOP_LEVEL: &[&str] = &[
        "doctor",
        "daemon",
        "config",
        "secrets",
        "toolchain",
        "sdk",
        "account",
        "provision",
        "sign",
        "device",
        "app",
        "install",
        "launch",
        "process",
        "forward",
        "logs",
        "screenshot",
        "mirror",
        "tap",
        "swipe",
        "button",
        "type",
        "files",
        "perf",
        "location",
        "build",
        "clean",
        "build-settings",
        "project",
        "inspect",
        "lint",
        "package",
        "assets",
        "strings",
        "ib",
        "run",
        "sessions",
        "stop",
        "relaunch",
        "reload",
        "debug",
        "crash",
        "symbols",
        "flutter",
        "xcode",
        "test",
        "ship",
        "mcp",
        "approvals",
        "approve",
        "deny",
        "audit",
        "self",
        "completions",
        "bugreport",
        "cache",
        "oracle",
    ];

    #[test]
    fn the_command_tree_is_valid_and_complete() {
        let root = root_command();
        root.clone().debug_assert();
        let names: BTreeSet<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        let expected: BTreeSet<&str> = TOP_LEVEL.iter().copied().collect();
        assert_eq!(names, expected);
        assert_eq!(root.get_subcommands().count(), TOP_LEVEL.len(), "a top-level command is registered twice");
    }

    #[test]
    fn every_top_level_command_has_one_owning_area() {
        let owners = cmd::owners();
        assert_eq!(owners.len(), TOP_LEVEL.len());
        assert!(owners.iter().any(|(c, area)| c == "install" && *area == "app"));
        assert!(owners.iter().any(|(c, area)| c == "sessions" && *area == "run"));
        assert!(owners.iter().any(|(c, area)| c == "self" && *area == "self_update"));
    }

    #[test]
    fn stubs_accept_any_arguments_and_report_not_implemented() {
        let m = root_command()
            .try_get_matches_from(["russet", "--json", "device", "list", "--udid", "x"])
            .expect("stub commands accept any arguments");
        assert_eq!(m.subcommand_name(), Some("device"));
        let err = cmd::not_implemented(&m).expect_err("stubs never succeed");
        assert_eq!(err.code(), codes::core::NOT_IMPLEMENTED);
    }
}
