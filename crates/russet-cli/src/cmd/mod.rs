// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The command seam (ARCHITECTURE §5.3 `russet-cli`, §8.7).
//!
//! Seam file: lead-only after W1 merges (PLAN §0 rule 2). The list of area modules is fixed here; each area owns
//! its `cmd/<area>.rs` (PLAN §3), which exports `pub fn command() -> clap::Command` and
//! `pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode>`.
//!
//! An area with several top-level commands (`app`, `install`, `launch`) returns them wrapped in [`group`]; every
//! command of a group becomes a top-level command. [`run()`] passes the *root* matches to the owning area, so
//! `m.subcommand()` names the command in both cases.

use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::CliCtx;

// Area modules: fixed by W1; each is owned by its area (PLAN §3). `flutter_run` (W18) is the run half of
// `russet flutter`, called by W17's `flutter.rs`; it is not a top-level area.
pub mod account;
pub mod app;
pub mod approvals;
pub mod assets;
pub mod audit;
pub mod bugreport;
pub mod build;
pub mod cache;
pub mod completions;
pub mod config;
pub mod crash;
pub mod daemon;
pub mod debug;
pub mod device;
pub mod doctor;
pub mod files;
pub mod flutter;
pub mod flutter_run;
pub mod forward;
pub mod ib;
pub mod input;
pub mod inspect;
pub mod location;
pub mod logs;
pub mod mcp;
pub mod oracle;
pub mod package;
pub mod perf;
pub mod process;
pub mod project;
pub mod provision;
pub mod run;
pub mod screen;
pub mod sdk;
pub mod secrets;
pub mod self_update;
pub mod ship;
pub mod sign;
pub mod symbols;
pub mod test;
pub mod toolchain;
pub mod xcode;

/// Hidden marker argument of a [`group`].
const GROUP_MARKER: &str = "russet-area-group";

macro_rules! areas {
    ($($module:ident),* $(,)?) => {
        /// Every top-level command, in help order (the order of ARCHITECTURE §8.7).
        pub fn commands() -> Vec<Command> {
            let mut out = Vec::new();
            $(out.extend(top_level($module::command()));)*
            out.into_iter().enumerate().map(|(i, c)| c.display_order(i)).collect()
        }

        /// Runs the top-level command named by `m.subcommand()` through the area that owns it.
        pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
            let Some(name) = m.subcommand_name() else {
                return Err(Error::new(codes::core::INVALID_ARGUMENT, "no command given").with_hint("Run `russet --help`."));
            };
            $(
                if top_level($module::command()).iter().any(|c| c.get_name() == name) {
                    return $module::run(m, cx).await;
                }
            )*
            Err(Error::new(codes::core::NOT_FOUND, format!("unknown command `{name}`")).with_hint("Run `russet --help`."))
        }

        /// `(top-level command, owning area module)` pairs, in help order.
        pub fn owners() -> Vec<(String, &'static str)> {
            let mut out = Vec::new();
            $(
                for c in top_level($module::command()) {
                    out.push((c.get_name().to_owned(), stringify!($module)));
                }
            )*
            out
        }
    };
}

areas!(
    doctor,
    daemon,
    config,
    secrets,
    toolchain,
    sdk,
    account,
    provision,
    sign,
    device,
    app,
    process,
    forward,
    logs,
    screen,
    input,
    files,
    perf,
    location,
    build,
    project,
    inspect,
    package,
    assets,
    ib,
    run,
    debug,
    crash,
    symbols,
    flutter,
    xcode,
    test,
    ship,
    mcp,
    approvals,
    audit,
    self_update,
    completions,
    bugreport,
    cache,
    oracle,
);

/// Wraps several top-level commands of one area (for example `app`, `install` and `launch`).
pub fn group(area: &'static str, commands: impl IntoIterator<Item = Command>) -> Command {
    Command::new(area)
        .subcommands(commands)
        .arg(Arg::new(GROUP_MARKER).long(GROUP_MARKER).hide(true).action(ArgAction::SetTrue))
}

fn top_level(c: Command) -> Vec<Command> {
    if c.get_arguments().any(|a| a.get_id().as_str() == GROUP_MARKER) {
        c.get_subcommands().cloned().collect()
    } else {
        vec![c]
    }
}

/// An F0 stub command: it accepts any arguments so that running it reports `core.notImplemented`.
pub fn stub(name: &'static str, about: &'static str) -> Command {
    Command::new(name)
        .about(about)
        .arg(Arg::new("args").num_args(0..).trailing_var_arg(true).allow_hyphen_values(true).hide(true))
}

/// The F0 result of every command: `core.notImplemented` (exit 1).
pub fn not_implemented(m: &ArgMatches) -> Result<ExitCode> {
    let name = m.subcommand_name().unwrap_or("russet");
    Err(Error::new(codes::core::NOT_IMPLEMENTED, format!("`russet {name}` is not implemented yet"))
        .with_hint("This command lands in a later workstream; see docs/PLAN.md."))
}
