// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `russet` CLI (ARCHITECTURE §5.3 `russet-cli`, §8.7; core §5.9): multi-call dispatch, the clap tree,
//! renderers and `--json`.
//!
//! - **Multi-call:** `russet <command>` is the CLI; the same file invoked as `russetd` is `russet daemon run`.
//! - **The daemon link** ([`DaemonLink`], `CliCtx.peer`) connects on the first call: to the running daemon
//!   (autostarting it from this executable, ARCHITECTURE §3.3), or to an in-process daemon with `--no-daemon` or
//!   `RUSSET_DAEMON=inproc`. Commands that need no daemon (`completions`, `daemon path`, `daemon run`) never
//!   connect.
//! - **Confirmations** (ARCHITECTURE §7.5): the CLI answers `ui.confirm` on its TTY, or at once with `--yes`;
//!   `ui.prompt` only on a TTY (typed confirmations are never pre-confirmed by `--yes`).
//! - **Output:** human text on stdout and diagnostics on stderr; with `--json` one JSON document (or NDJSON for
//!   streams) on stdout, errors as `{"type":"error","error":{…}}` (ARCHITECTURE §13.1).
//! - **Exit codes** come from the error's code (ARCHITECTURE §13.4); clap's usage errors exit 2.

// Command modules of other areas are F0 stubs whose `cx` parameter is unused until their owners fill them.
#![allow(unused_variables)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Command;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result};
use russet_core::rpc::{ClientKind, Method};
use russet_ipc::{CallOptions, ClientInfo, Peer, RpcError};
use russet_protocol::initialize::InitializeResult;
use serde::Serialize;
use tokio::sync::OnceCell;

pub mod cmd;
pub mod global;
pub mod render;
pub mod spawn;

pub use global::GlobalArgs;
pub use render::Reporter;

/// The connection to the daemon, made on first use (module docs).
pub struct DaemonLink {
    global: GlobalArgs,
    connection: OnceCell<(Peer, InitializeResult)>,
    in_process: tokio::sync::Mutex<Option<russetd::InProcess>>,
}

impl std::fmt::Debug for DaemonLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonLink").field("connected", &self.connection.get().is_some()).finish()
    }
}

/// Whether the daemon runs in this process (`--no-daemon`, `RUSSET_DAEMON=inproc`).
pub fn wants_in_process(global: &GlobalArgs) -> bool {
    global.no_daemon || russet_ipc::in_process_requested()
}

impl DaemonLink {
    /// A link for these flags; nothing connects yet.
    pub fn new(global: GlobalArgs) -> Self {
        Self { global, connection: OnceCell::new(), in_process: tokio::sync::Mutex::new(None) }
    }

    /// The client this CLI presents to the daemon: kind `cli`, its TTY and `--yes`, the project root and the
    /// shared reservation of `RUSSET_LEASE` (ARCHITECTURE §6.3, §7.4).
    pub fn client_info(&self) -> ClientInfo {
        let tty = russet_core::platform::stdin_is_tty() && russet_core::platform::stderr_is_tty();
        let mut c = ClientInfo::new(ClientKind::Cli, "russet");
        c.capabilities.interactive = tty;
        c.capabilities.confirm = tty || self.global.yes;
        c.capabilities.prompt = tty;
        c.capabilities.open_url = tty;
        if let Some(p) = &self.global.project {
            c.workspace_roots = vec![std::path::absolute(p).unwrap_or_else(|_| p.clone()).display().to_string()];
        }
        c.shared_lease = std::env::var("RUSSET_LEASE")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .and_then(|v| russet_core::ids::LeaseId::parse(v.trim()).ok());
        c
    }

    /// The daemon connection, starting the daemon when needed.
    pub async fn get(&self) -> Result<&Peer> {
        let (peer, _) = self.connection.get_or_try_init(|| self.connect(true)).await?;
        Ok(peer)
    }

    /// The daemon connection only if a daemon already runs (`daemon stop`, `bugreport`); `ipc.daemonUnavailable`
    /// otherwise.
    pub async fn existing(&self) -> Result<&Peer> {
        let (peer, _) = self.connection.get_or_try_init(|| self.connect(false)).await?;
        Ok(peer)
    }

    /// The `initialize` result, once connected.
    pub fn initialize_result(&self) -> Option<&InitializeResult> {
        self.connection.get().map(|(_, r)| r)
    }

    async fn connect(&self, spawn: bool) -> Result<(Peer, InitializeResult)> {
        let dirs = RussetDirs::from_env()?;
        let client = self.client_info();
        let (peer, result) = if wants_in_process(&self.global) {
            let opts = russetd::DaemonOptions::from_env(dirs)?;
            let (daemon, peer, result) = russetd::InProcess::start_with(opts, &client).await?;
            *self.in_process.lock().await = Some(daemon);
            (peer, result)
        } else if spawn {
            russet_ipc::connect_or_spawn(&dirs, client, spawn::spawn_spec(None)?).await?
        } else {
            let endpoint = russet_ipc::Endpoint::from_env(&dirs)?;
            russet_ipc::connect_with(&endpoint, client, russet_ipc::client::FIRST_TRY).await?
        };
        render::prompts::install(&peer, &self.global);
        Ok((peer, result))
    }

    /// Calls method `M` on the daemon.
    pub async fn call<M: Method>(&self, params: M::Params) -> Result<M::Result> {
        self.get().await?.call::<M>(params).await
    }

    /// Calls method `M` with options.
    pub async fn call_with<M: Method>(&self, params: M::Params, opts: CallOptions) -> Result<M::Result> {
        self.get().await?.call_with::<M>(params, opts).await
    }

    /// Calls `method` with raw JSON params.
    pub async fn call_raw(
        &self,
        method: &str,
        params: serde_json::Value,
        opts: CallOptions,
    ) -> Result<serde_json::Value> {
        self.get().await?.call_raw(method, params, opts).await.map_err(|e: RpcError| e.to_error())
    }

    /// Waits until the daemon connection closes or the user interrupts (Ctrl-C); returns at once when not
    /// connected. Commands whose effect lives as long as their connection (forwards, reservations) end with it.
    pub async fn closed(&self) {
        let Some((peer, _)) = self.connection.get() else { return };
        tokio::select! {
            () = peer.closed() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }

    /// Stops an in-process daemon (the CLI exits).
    pub async fn finish(&self) {
        if let Some(d) = self.in_process.lock().await.take() {
            let _ = d.stop().await;
        }
    }
}

/// What every command gets: the daemon link, the reporter and the global flags.
pub struct CliCtx {
    /// The daemon connection, made on first use ([`DaemonLink::get`]).
    pub peer: DaemonLink,
    pub reporter: Arc<dyn render::Reporter>,
    pub global: global::GlobalArgs,
}

impl CliCtx {
    /// A context for `global`, with the matching reporter.
    pub fn new(global: GlobalArgs) -> Self {
        let reporter = render::reporter_for(&global);
        Self { peer: DaemonLink::new(global.clone()), reporter, global }
    }

    /// Prints a command's result: the JSON document with `--json`, else the human text `human` makes of it.
    pub fn output<T: Serialize>(&self, value: &T, human: impl FnOnce(&T) -> String) -> Result<()> {
        if self.reporter.is_json() {
            let v =
                serde_json::to_value(value).map_err(|e| Error::internal(format!("cannot render the result: {e}")))?;
            self.reporter.result(&v);
        } else if !self.global.quiet {
            self.reporter.human(&human(value));
        }
        Ok(())
    }

    /// Calls the long-running method `M` (ARCHITECTURE §7.6) as area commands should
    /// ([`render::job`]): progress on the reporter, Ctrl-C cancels the job (exit 130), and in human mode
    /// (not `-q`) the job's log lines are printed as they arrive. Put `background: true` in the params for
    /// `--background` (the result is then `{jobId}`).
    pub async fn call_long<M: Method>(&self, params: M::Params) -> Result<M::Result> {
        let logs = !self.reporter.is_json() && !self.global.quiet;
        self.call_long_with::<M>(params, logs).await
    }

    /// [`CliCtx::call_long`], choosing whether the job's log lines are printed.
    pub async fn call_long_with<M: Method>(&self, params: M::Params, logs: bool) -> Result<M::Result> {
        let peer = self.peer.get().await?;
        render::job::call_long::<M, _, _>(peer, Arc::clone(&self.reporter), params, logs, || async {
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        })
        .await
    }
}

/// The whole command tree: global flags plus every area's commands (ARCHITECTURE §8.7).
pub fn root_command() -> Command {
    let root = Command::new("russet")
        .version(russet_core::build_info::BUILD_ID)
        .about("Build, run, debug and ship iOS apps from Linux and Windows, with a real iPhone and no Mac")
        .subcommand_required(true)
        .arg_required_else_help(true);
    global::args(root).subcommands(cmd::commands())
}

/// Multi-call: the file invoked as `russetd` is `russet daemon run`.
pub fn multicall(args: Vec<OsString>) -> Vec<OsString> {
    let stem = args
        .first()
        .and_then(|a| std::path::Path::new(a).file_stem())
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if stem == "russetd" {
        let mut out = vec![OsString::from("russet"), OsString::from("daemon"), OsString::from("run")];
        out.extend(args.into_iter().skip(1));
        out
    } else {
        args
    }
}

/// Whether the parsed command runs the daemon itself (`daemon run`): it needs the multi-thread runtime and
/// installs its own logging.
fn is_daemon_run(m: &clap::ArgMatches) -> bool {
    matches!(m.subcommand(), Some(("daemon", sub)) if sub.subcommand_name() == Some("run"))
}

/// The process entry point: parses the command line and dispatches.
pub fn main_entry() -> ExitCode {
    // One rustls provider for the whole binary: ring (ARCHITECTURE §16.6 [D 21]).
    let _ = rustls::crypto::ring::default_provider().install_default();
    run_args(multicall(std::env::args_os().collect()))
}

/// Runs the CLI on `args` (tests call it too).
pub fn run_args(args: Vec<OsString>) -> ExitCode {
    let matches = match root_command().try_get_matches_from(args) {
        Ok(m) => m,
        Err(e) => {
            let _ = e.print();
            return ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(2));
        }
    };
    let global = GlobalArgs::from_matches(&matches);
    let daemon_run = is_daemon_run(&matches);
    if !daemon_run {
        let _ = tracing::subscriber::set_global_default(russetd::daemon::logging::cli_subscriber(global.verbose));
    }
    let multi = daemon_run || wants_in_process(&global);
    let runtime = if multi {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(std::thread::available_parallelism().map_or(2, |n| n.get().min(4)))
            .thread_name("russetd-worker")
            .enable_all()
            .build()
    } else {
        tokio::runtime::Builder::new_current_thread().enable_all().build()
    };
    let runtime = match runtime {
        Ok(r) => r,
        Err(e) => {
            render::print_error_line(&Error::io("starting the runtime", e), global.json);
            return ExitCode::from(1);
        }
    };
    let cx = Arc::new(CliCtx::new(global));
    let outcome = runtime.block_on(async {
        let r = cmd::run(&matches, &cx).await;
        cx.peer.finish().await;
        r
    });
    match outcome {
        Ok(code) => code,
        Err(e) => {
            cx.reporter.error(&e);
            ExitCode::from(exit_code_of(&e))
        }
    }
}

/// The exit status of an error (ARCHITECTURE §13.4).
pub fn exit_code_of(e: &Error) -> u8 {
    e.code().exit_code()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use russet_core::error::codes;

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

    #[test]
    fn russetd_is_daemon_run() {
        let args = multicall(vec!["/usr/bin/russetd".into(), "--foreground".into()]);
        assert_eq!(args, vec![OsString::from("russet"), "daemon".into(), "run".into(), "--foreground".into()]);
        let same = multicall(vec!["russet".into(), "daemon".into(), "status".into()]);
        assert_eq!(same.len(), 3);
        let m = root_command().try_get_matches_from(args).expect("parses");
        assert!(is_daemon_run(&m));
    }

    #[test]
    fn exit_codes_follow_the_table_of_section_13_4() {
        let cases = [
            (codes::core::INTERNAL, 1),
            (codes::core::INVALID_ARGUMENT, 1),
            (codes::core::NOT_IMPLEMENTED, 1),
            (codes::core::NOT_FOUND, 3),
            (codes::device::NOT_FOUND, 3),
            (codes::config::INVALID, 4),
            (codes::toolchain::MISSING, 4),
            (codes::sdk::MISSING, 4),
            (codes::core::DISK_FULL, 4),
            (codes::secrets::UNAVAILABLE, 5),
            (codes::account::ASC_UNAUTHORIZED, 5),
            (codes::device::LOCKED, 6),
            (codes::device::LIVE_BACKEND_REFUSED, 6),
            (codes::app::INSTALL_FAILED, 6),
            (codes::build::COMPILE_FAILED, 7),
            (codes::sign::CODESIGN_FAILED, 8),
            (codes::provision::CERT_LIMIT, 8),
            (codes::app::LAUNCH_FAILED, 9),
            (codes::debug::ATTACH_DENIED, 9),
            (codes::ship::UPLOAD_FAILED, 10),
            (codes::perm::DENIED, 11),
            (codes::perm::APPROVAL_REQUIRED, 11),
            (codes::mcp::HTTP_UNAUTHORIZED, 11),
            (codes::ipc::DAEMON_UNAVAILABLE, 12),
            (codes::ipc::BUILD_MISMATCH, 12),
            (codes::core::CANCELLED, 130),
        ];
        for (code, exit) in cases {
            assert_eq!(exit_code_of(&Error::new(code, "x")), exit, "{}", code.as_str());
        }
    }
}
