// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet debug` (ARCHITECTURE §7.6, §8.7, §17.1; RD §20). Owner: W7 (Wave 1), W18 (`jit-bench`, Wave 3).
//!
//! `russet debug` calls `debug.prepare` (build → sign → install-if-changed → suspended launch → broker
//! pre-attach), appends the `--break` breakpoints and `process continue` to the daemon's Python-free
//! `attach.lldb`, and runs LLDB with the resolver's environment:
//!
//! - interactive: `lldb -s session.lldb` in the terminal; Ctrl-C belongs to LLDB (it interrupts the app), so
//!   `russet` ignores SIGINT while LLDB runs; when LLDB quits, an owned session is stopped;
//! - `--batch --command CMD…`: the script continues to the first stop, runs the commands there, and ends with
//!   `process detach` (`--detach`) or `process kill` [D 84]; `--json` prints `{sessionId, pid, stop{reason,
//!   threadId, frame{function, file, line}}, commands[{command, output}], detached, exitedAt?}`. `--timeout SECS`
//!   (default 120, 0 = none) bounds the run, and Ctrl-C ends it (exit 130): either ends the session under LLDB
//!   (`run.stop`, kill, or detach with `--detach`). A batch that never stopped fails (`debug.processExited`,
//!   `debug.lldbUnsupported`, `debug.brokerLost`, `debug.batchTimeout`), with the transcript in
//!   `details.result`;
//! - `--detach` makes the session `detached` (`debug.prepare {sessionLease: detached}`), so it outlives the CLI
//!   and stays `running` with `debugger: detached` after LLDB's `D`;
//! - `--print-config` prints the LLDB command line, environment and commands instead of running LLDB;
//! - `russet debug prepare --json` prints `debug.prepare`'s result (`PreparedDebug`). With `--detach` it exits and
//!   leaves a detached session (§7.6); without, it holds the owned session until it ends (for example when the
//!   debugger that connected quits) or until Ctrl-C, which stops it.

use std::path::{Path, PathBuf};
use std::process::{ExitCode, ExitStatus};
use std::time::Duration;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::common::{AttachMode, LogLevel, LogSource, SessionLease, SessionState};
use russet_protocol::debug::{
    DebugAttach, DebugAttachParams, DebugFrontend, DebugPrepare, DebugPrepareParams, DebugPrepareResult,
    DebuggerFlavor, JitPolicy, SymbolTier,
};
use russet_protocol::run::{
    OutputFilter, RunList, RunListParams, RunOutputQuery, RunOutputQueryParams, RunStop, RunStopParams, StopMode,
};
use russetd::pipeline::lldb::{self, BatchEnd, BatchLimits, BatchSpec, BatchTranscript, Breakpoint};

use super::run::{Interrupts, cancelled, interrupts, pipeline_args, project_dir, session_error, start_params};
use crate::CliCtx;

/// The options `russet debug` and `russet debug prepare` share.
fn debug_args(c: Command) -> Command {
    c.arg(
        Arg::new("stop-on-entry")
            .long("stop-on-entry")
            .action(ArgAction::SetTrue)
            .help("Stay stopped after the attach"),
    )
    .arg(
        Arg::new("attach-mode")
            .long("attach-mode")
            .value_name("MODE")
            .value_parser(["broker", "legacy-async"])
            .help("broker (default), or the M1 fallback legacy-async"),
    )
    .arg(
        Arg::new("jit")
            .long("jit")
            .value_name("MODE")
            .value_parser(["auto", "native", "lldb-python", "off"])
            .help("JIT servicer (Flutter)"),
    )
    .arg(
        Arg::new("symbols")
            .long("symbols")
            .value_name("TIER")
            .value_parser(["auto", "none", "core", "full"])
            .help("Device symbols"),
    )
    .arg(Arg::new("flavor").long("flavor").value_name("F").value_parser(["swift", "llvm"]).help("LLDB flavor"))
    .arg(Arg::new("detach").long("detach").action(ArgAction::SetTrue).help("Leave the app running when LLDB is done"))
}

/// The `debug` command.
pub fn command() -> Command {
    let debug = debug_args(pipeline_args(Command::new("debug")))
        .about("Build, install, launch and debug with LLDB [--break FILE:LINE]… [--batch] [--detach]; debug prepare, jit-bench")
        .args_conflicts_with_subcommands(true)
        .arg(Arg::new("break").long("break").value_name("FILE:LINE").action(ArgAction::Append).help("A breakpoint"))
        .arg(Arg::new("attach").long("attach").action(ArgAction::SetTrue).help("Attach to a running app (W11)"))
        .arg(Arg::new("pid").long("pid").value_name("N").requires("attach").help("With --attach: the process id"))
        .arg(Arg::new("bundle").long("bundle").value_name("B").requires("attach").help("With --attach: the bundle id"))
        .arg(Arg::new("print-config").long("print-config").action(ArgAction::SetTrue).help("Print the LLDB plan; do not run LLDB"))
        .arg(Arg::new("batch").long("batch").action(ArgAction::SetTrue).help("Run non-interactively"))
        .arg(
            Arg::new("command")
                .long("command")
                .value_name("CMD")
                .action(ArgAction::Append)
                .requires("batch")
                .help("With --batch: an LLDB command to run at the first stop"),
        )
        .arg(
            Arg::new("timeout")
                .long("timeout")
                .value_name("SECS")
                .value_parser(clap::value_parser!(u64))
                .requires("batch")
                .help("With --batch: give up after SECS seconds (default 120; 0: never)"),
        )
        .arg(Arg::new("lldb").long("lldb").value_name("PATH").help("Use this lldb instead of the resolved one"));
    debug
        .subcommand(
            debug_args(pipeline_args(Command::new("prepare")))
                .about("Run the pipeline up to the broker attach and print the plan; hold the session (or --detach)"),
        )
        .subcommand(super::stub("jit-bench", "Per-stop JIT latency on the device (W18)"))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("debug", d)) = m.subcommand() else {
        return super::not_implemented(m);
    };
    match d.subcommand() {
        Some(("prepare", p)) => prepare(p, cx).await,
        Some(("jit-bench", _)) => {
            Err(Error::new(codes::core::NOT_IMPLEMENTED, "`russet debug jit-bench` lands with W18")
                .with_hint("It needs the JIT servicer of Wave 3."))
        }
        _ => debug(d, cx).await,
    }
}

/// `debug.prepare`'s params from the flags.
pub fn prepare_params(
    m: &ArgMatches,
    project: &Path,
    device: Option<String>,
    lease: SessionLease,
) -> Result<DebugPrepareParams> {
    let one = |name: &str| m.try_get_one::<String>(name).ok().flatten().cloned();
    Ok(DebugPrepareParams {
        run: start_params(m, project, device, lease)?,
        stop_on_entry: m.try_get_one::<bool>("stop-on-entry").ok().flatten().copied().filter(|b| *b),
        frontend: Some(DebugFrontend::LldbCli),
        jit: one("jit").map(|s| match s.as_str() {
            "native" => JitPolicy::Native,
            "lldb-python" => JitPolicy::LldbPython,
            "off" => JitPolicy::Off,
            _ => JitPolicy::Auto,
        }),
        symbols: one("symbols").map(|s| match s.as_str() {
            "none" => SymbolTier::None,
            "core" => SymbolTier::Core,
            "full" => SymbolTier::Full,
            _ => SymbolTier::Auto,
        }),
        flavor: one("flavor").map(|s| if s == "llvm" { DebuggerFlavor::Llvm } else { DebuggerFlavor::Swift }),
        attach_mode: one("attach-mode")
            .map(|s| if s == "legacy-async" { AttachMode::LegacyAsync } else { AttachMode::Broker }),
    })
}

/// The `--break` breakpoints.
pub fn breakpoints(m: &ArgMatches) -> Result<Vec<Breakpoint>> {
    m.get_many::<String>("break")
        .into_iter()
        .flatten()
        .map(|b| {
            Breakpoint::parse(b).ok_or_else(|| {
                Error::new(codes::core::INVALID_ARGUMENT, format!("`--break {b}` is not FILE:LINE"))
                    .with_hint("Write `--break ContentView.swift:12`.")
            })
        })
        .collect()
}

/// The script `russet debug` runs: the daemon's init and attach commands, the breakpoints, `process continue`
/// and, for `--batch`, the stop probe, the commands and the final detach or kill.
pub fn session_script(
    prepared: &DebugPrepareResult,
    bps: &[Breakpoint],
    stop_on_entry: bool,
    batch: Option<&BatchSpec>,
) -> Vec<String> {
    lldb::session_script(&prepared.init_commands, &prepared.attach_commands, bps, stop_on_entry, batch)
}

/// Where `session.lldb` goes: next to the daemon's `attach.lldb`, or under the project's `.russet/debug/<sid>/`.
fn script_path(prepared: &DebugPrepareResult, project: &Path) -> PathBuf {
    match &prepared.attach_script {
        Some(a) => Path::new(a).with_file_name("session.lldb"),
        None => project.join(".russet").join("debug").join(prepared.session_id.as_str()).join("session.lldb"),
    }
}

/// The `--json` result of `--batch` (ARCHITECTURE §8.7).
pub fn batch_result(
    prepared: &DebugPrepareResult,
    t: &BatchTranscript,
    exited_at: Option<String>,
) -> serde_json::Value {
    let mut v = serde_json::json!({
        "sessionId": prepared.session_id,
        "pid": prepared.pid,
        "commands": t.commands,
        "detached": t.detached,
    });
    if let Some(stop) = &t.stop {
        v["stop"] = serde_json::to_value(stop).unwrap_or_default();
    }
    if let Some(at) = exited_at {
        v["exitedAt"] = serde_json::Value::String(at);
    }
    v
}

/// `--print-config`: what LLDB would run.
pub fn print_config(
    prepared: &DebugPrepareResult,
    program: &Path,
    args: &[String],
    script: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "sessionId": prepared.session_id,
        "pid": prepared.pid,
        "lldb": { "program": program, "args": args, "env": prepared.lldb.env },
        "broker": prepared.broker,
        "script": script,
        "attachMode": prepared.attach_mode,
        "dapConfig": prepared.dap_config,
    })
}

async fn prepare(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let project = project_dir(cx)?;
    let detach = m.get_flag("detach");
    let lease = if detach { SessionLease::Detached } else { SessionLease::Owned };
    let mut params = prepare_params(m, &project, cx.global.device.clone(), lease)?;
    params.frontend = Some(DebugFrontend::None);
    let prepared = cx.peer.call::<DebugPrepare>(params).await?;
    let v = serde_json::to_value(&prepared).map_err(|e| Error::new(codes::core::INTERNAL, e.to_string()))?;
    cx.reporter.result(&v);
    if !detach {
        hold(cx, &prepared).await?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Keeps an owned session (and so its app) alive until it ends, or until Ctrl-C, which stops it (exit 130). A
/// session that fails (for example `debug.handoffTimeout`: nobody connected within 60 s) returns its error.
async fn hold(cx: &CliCtx, prepared: &DebugPrepareResult) -> Result<()> {
    let mut interrupts = interrupts()?;
    let sid = &prepared.session_id;
    loop {
        let list = cx.peer.call::<RunList>(RunListParams {}).await?;
        match list.sessions.into_iter().find(|s| &s.session_id == sid) {
            Some(s) if s.ended_at.is_none() => {}
            Some(s) if matches!(s.state, SessionState::Failed | SessionState::Lost) => {
                return Err(last_session_error(cx, prepared).await.unwrap_or_else(|| {
                    Error::new(codes::debug::BROKER_LOST, format!("the debug session ended {:?}", s.state))
                        .with_hint("See `russet logs --session` for what happened.")
                }));
            }
            _ => return Ok(()),
        }
        tokio::select! {
            () = interrupts.recv() => {
                let _ = cx.peer.call::<RunStop>(RunStopParams { session_id: sid.clone(), mode: None }).await;
                return Err(cancelled());
            }
            () = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
    }
}

/// The error Russet reported in the session's output, if any.
async fn last_session_error(cx: &CliCtx, prepared: &DebugPrepareResult) -> Option<Error> {
    let filter = OutputFilter {
        sources: Some(vec![LogSource::Russet]),
        min_level: Some(LogLevel::Error),
        ..OutputFilter::default()
    };
    let q = RunOutputQueryParams {
        session_id: prepared.session_id.clone(),
        after_seq: None,
        since_ms: None,
        limit: None,
        filter: Some(filter),
    };
    let out = cx.peer.call::<RunOutputQuery>(q).await.ok()?;
    out.entries.iter().rev().find_map(russetd::pipeline::logs::parse_error_entry).map(|e| session_error(&e))
}

/// Runs the interactive LLDB until it exits. LLDB shares the terminal and `russet`'s process group, so a Ctrl-C
/// reaches it (it interrupts the app); `russet` ignores SIGINT meanwhile. Without that, `russet` would die and
/// leave LLDB orphaned, and its owned session would be stopped under the user 2 s later.
pub async fn run_interactive(cmd: &mut tokio::process::Command) -> std::io::Result<ExitStatus> {
    // Watch for SIGINT before LLDB starts: from then on it never terminates `russet`.
    let mut sigint = Interrupts::new()?;
    let mut child = cmd.kill_on_drop(true).spawn()?;
    loop {
        tokio::select! {
            status = child.wait() => return status,
            () = sigint.recv() => {}
        }
    }
}

async fn debug(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let detach = m.get_flag("detach");
    let lease = if detach { SessionLease::Detached } else { SessionLease::Owned };
    let attaching = m.get_flag("attach");
    // Attaching to a running app needs no project; launching does.
    let project = if attaching {
        project_dir(cx).unwrap_or_else(|_| std::env::current_dir().unwrap_or_default())
    } else {
        project_dir(cx)?
    };
    let (prepared, stop_on_entry) = if attaching {
        let params = DebugAttachParams {
            udid: cx.global.device.clone(),
            pid: m.get_one::<String>("pid").and_then(|p| p.parse::<u32>().ok()),
            bundle_id: m.get_one::<String>("bundle").cloned(),
            frontend: Some(DebugFrontend::LldbCli),
            project: project_dir(cx).ok().map(|p| p.to_string_lossy().into_owned()),
            session_lease: Some(lease),
        };
        (cx.peer.call::<DebugAttach>(params).await?, m.get_flag("stop-on-entry"))
    } else {
        let params = prepare_params(m, &project, cx.global.device.clone(), lease)?;
        let soe = params.stop_on_entry.unwrap_or(false);
        (cx.peer.call::<DebugPrepare>(params).await?, soe)
    };
    let bps = breakpoints(m)?;
    let batch = m.get_flag("batch").then(|| BatchSpec {
        commands: m.get_many::<String>("command").into_iter().flatten().cloned().collect(),
        end: if detach { BatchEnd::Detach } else { BatchEnd::Kill },
    });
    let limits = batch_limits(m);
    let script = session_script(&prepared, &bps, stop_on_entry, batch.as_ref());
    let path = script_path(&prepared, &project);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::new(codes::core::IO, format!("creating {}: {e}", dir.display())))?;
    }
    std::fs::write(&path, lldb::script_text(&script))
        .map_err(|e| Error::new(codes::core::IO, format!("writing {}: {e}", path.display())))?;
    let program = m.get_one::<String>("lldb").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(&prepared.lldb.path));
    let args = if batch.is_some() { lldb::batch_args(&path) } else { lldb::interactive_args(&path) };
    if m.get_flag("print-config") {
        cx.reporter.result(&print_config(&prepared, &program, &args, &script));
        return Ok(ExitCode::SUCCESS);
    }
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&args).envs(&prepared.lldb.env);
    let Some(batch) = &batch else {
        run_interactive(&mut cmd).await.map_err(|e| lldb_spawn_error(&program, e))?;
        if !detach {
            // LLDB detaches on quit after `process connect`; an owned session's app is killed, as without --detach.
            stop_session(cx, &prepared, StopMode::Kill).await;
        }
        return Ok(ExitCode::SUCCESS);
    };
    let mut sigint = interrupts()?;
    let end_mode = if detach { StopMode::Detach } else { StopMode::Kill };
    let run = lldb::run_batch(cmd, limits, sigint.recv(), |_| stop_session(cx, &prepared, end_mode))
        .await
        .map_err(|e| lldb_spawn_error(&program, e))?;
    let t = lldb::parse_batch_output(&run.stdout, &batch.commands);
    let exited_at = t.exited.then(russetd::pipeline::logs::now_rfc3339);
    let result = batch_result(&prepared, &t, exited_at);
    if !detach {
        stop_session(cx, &prepared, StopMode::Kill).await;
    }
    if !cx.global.json {
        let mut text = run.stdout.clone();
        text.push_str(&run.stderr);
        cx.reporter.result(&serde_json::Value::String(text));
    }
    if let Some(e) = lldb::batch_failure(&t, &run) {
        let mut details = e.to_data().details.unwrap_or_else(|| serde_json::json!({}));
        details["result"] = result;
        return Err(e.with_details(details));
    }
    if cx.global.json {
        cx.reporter.result(&result);
    }
    Ok(ExitCode::SUCCESS)
}

/// `--timeout` (with `--batch`): 120 s by default, `0` for none.
pub fn batch_limits(m: &ArgMatches) -> BatchLimits {
    let timeout = match m.try_get_one::<u64>("timeout").ok().flatten() {
        Some(0) => None,
        Some(secs) => Some(Duration::from_secs(*secs)),
        None => Some(lldb::BATCH_TIMEOUT),
    };
    BatchLimits { timeout, ..BatchLimits::default() }
}

/// Ends the session: `kill`, or `detach` (the app keeps running untracked). Errors are ignored: the session may
/// already be over.
async fn stop_session(cx: &CliCtx, prepared: &DebugPrepareResult, mode: StopMode) {
    let params = RunStopParams { session_id: prepared.session_id.clone(), mode: Some(mode) };
    let _ = cx.peer.call::<RunStop>(params).await;
}

fn lldb_spawn_error(program: &Path, e: std::io::Error) -> Error {
    Error::new(codes::toolchain::MISSING, format!("cannot run {}: {e}", program.display()))
        .with_hint("Run `russet doctor` to check LLDB, or pass `--lldb PATH`.")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeMap;

    use russet_protocol::debug::{BrokerInfo, BrokerTransport, DebugToolComponent, DebugToolInfo, DebugToolProvenance};
    use russetd::pipeline::lldb::{BatchStop, CommandOutput, StopFrame};

    use super::*;

    fn parse(args: &[&str]) -> ArgMatches {
        crate::root_command().try_get_matches_from(args).unwrap()
    }

    fn debug_matches(m: &ArgMatches) -> &ArgMatches {
        match m.subcommand() {
            Some(("debug", d)) => d,
            other => panic!("{other:?}"),
        }
    }

    fn prepared() -> DebugPrepareResult {
        let tool = DebugToolInfo {
            component: DebugToolComponent::Lldb,
            version: "21.0.0".into(),
            path: "/usr/lib/swift/usr/bin/lldb".into(),
            provenance: DebugToolProvenance::Uv,
            env: BTreeMap::from([("PYTHONHOME".to_owned(), "/pbs".to_owned())]),
            ok: true,
            problems: vec![],
        };
        DebugPrepareResult {
            session_id: serde_json::from_value(serde_json::json!("ses_01J00000000000000000000000")).unwrap(),
            pid: 932,
            bundle_id: "dev.russet.test.probe".into(),
            lldb: tool.clone(),
            lldb_dap: tool,
            broker: BrokerInfo {
                transport: BrokerTransport::Unix,
                path: Some("/r/gdb.sock".into()),
                port: None,
                lldb_url: "unix-connect:///r/gdb.sock".into(),
            },
            init_commands: vec!["settings set target.load-cwd-lldbinit false".into()],
            attach_commands: vec![
                "platform select remote-ios".into(),
                "target create --arch arm64-apple-ios \"/p/.russet/build/debug-iphoneos/RussetProbe.app/RussetProbe\""
                    .into(),
                "process connect -p gdb-remote \"unix-connect:///r/gdb.sock\"".into(),
            ],
            dap_config: serde_json::json!({}),
            dap: None,
            attach_script: Some("/p/.russet/debug/ses_01J00000000000000000000000/attach.lldb".into()),
            attach_mode: AttachMode::Broker,
        }
    }

    #[test]
    fn the_w9_command_line_parses() {
        let m = parse(&[
            "russet",
            "debug",
            "--batch",
            "--detach",
            "--break",
            "ContentView.swift:13",
            "--command",
            "frame variable probeValue",
            "--json",
        ]);
        assert!(m.get_flag("json"));
        let d = debug_matches(&m);
        assert!(d.get_flag("batch") && d.get_flag("detach"));
        assert_eq!(breakpoints(d).unwrap(), vec![Breakpoint { file: "ContentView.swift".into(), line: 13 }]);
        let p = prepare_params(d, Path::new("/p"), None, SessionLease::Detached).unwrap();
        assert_eq!(p.run.session_lease, Some(SessionLease::Detached));
        assert_eq!(p.frontend, Some(DebugFrontend::LldbCli));
        assert_eq!(p.attach_mode, None);
        // --command needs --batch; a bad breakpoint is a usage error.
        assert!(crate::root_command().try_get_matches_from(["russet", "debug", "--command", "bt"]).is_err());
        let m = parse(&["russet", "debug", "--break", "nope"]);
        assert_eq!(breakpoints(debug_matches(&m)).unwrap_err().code(), codes::core::INVALID_ARGUMENT);
        let m = parse(&[
            "russet",
            "debug",
            "--attach-mode",
            "legacy-async",
            "--stop-on-entry",
            "--",
            "-RussetProbeCrash",
            "1",
        ]);
        let p = prepare_params(debug_matches(&m), Path::new("/p"), Some("u".into()), SessionLease::Owned).unwrap();
        assert_eq!(p.attach_mode, Some(AttachMode::LegacyAsync));
        assert_eq!(p.stop_on_entry, Some(true));
        assert_eq!(p.run.args.as_deref(), Some(&["-RussetProbeCrash".to_owned(), "1".to_owned()][..]));
        let m = parse(&["russet", "debug", "prepare", "--json"]);
        assert!(matches!(debug_matches(&m).subcommand(), Some(("prepare", _))));
    }

    #[test]
    fn debug_prepare_takes_the_debug_flags() {
        let m = parse(&[
            "russet",
            "debug",
            "prepare",
            "--detach",
            "--stop-on-entry",
            "--attach-mode",
            "legacy-async",
            "--flavor",
            "llvm",
            "--jit",
            "off",
            "--symbols",
            "none",
            "--json",
        ]);
        let Some(("prepare", p)) = debug_matches(&m).subcommand() else { panic!("no prepare") };
        assert!(p.get_flag("detach"));
        let params = prepare_params(p, Path::new("/p"), None, SessionLease::Detached).unwrap();
        assert_eq!(params.stop_on_entry, Some(true));
        assert_eq!(params.attach_mode, Some(AttachMode::LegacyAsync));
        assert_eq!(
            (params.flavor, params.jit, params.symbols),
            (Some(DebuggerFlavor::Llvm), Some(JitPolicy::Off), Some(SymbolTier::None))
        );
        // Batch-only flags stay on `russet debug`.
        assert!(crate::root_command().try_get_matches_from(["russet", "debug", "prepare", "--batch"]).is_err());
    }

    #[test]
    fn the_batch_time_limit() {
        let limit = |args: &[&str]| batch_limits(debug_matches(&parse(args))).timeout;
        assert_eq!(limit(&["russet", "debug", "--batch"]), Some(lldb::BATCH_TIMEOUT));
        assert_eq!(limit(&["russet", "debug", "--batch", "--timeout", "60"]), Some(Duration::from_secs(60)));
        assert_eq!(limit(&["russet", "debug", "--batch", "--timeout", "0"]), None);
        assert!(crate::root_command().try_get_matches_from(["russet", "debug", "--timeout", "5"]).is_err());
    }

    #[test]
    fn the_session_script_ends_as_asked_and_has_no_python() {
        let p = prepared();
        let bps = vec![Breakpoint { file: "ContentView.swift".into(), line: 13 }];
        let detach = BatchSpec { commands: vec!["frame variable probeValue".into()], end: BatchEnd::Detach };
        let s = session_script(&p, &bps, false, Some(&detach));
        assert_eq!(s.last().map(String::as_str), Some("process detach"));
        assert!(s.contains(&"process continue".to_owned()));
        assert!(!lldb::uses_python(&s));
        let interactive = session_script(&p, &bps, false, None);
        assert_eq!(interactive.last().map(String::as_str), Some("process continue"));
        assert_eq!(
            script_path(&p, Path::new("/p")),
            Path::new("/p/.russet/debug/ses_01J00000000000000000000000/session.lldb")
        );
    }

    #[test]
    fn the_batch_json_has_the_section_8_7_shape() {
        let p = prepared();
        let t = BatchTranscript {
            stop: Some(BatchStop {
                reason: "breakpoint".into(),
                description: Some("breakpoint 1.1".into()),
                thread_id: Some(0x1a2b),
                frame: StopFrame {
                    function: Some("ContentView.body.getter".into()),
                    file: Some("/p/Sources/ContentView.swift".into()),
                    line: Some(13),
                },
            }),
            commands: vec![CommandOutput {
                command: "frame variable probeValue".into(),
                output: "(Int) probeValue = 42".into(),
            }],
            detached: true,
            exited: false,
            exit_status: None,
        };
        let v = batch_result(&p, &t, None);
        assert_eq!(v["stop"]["reason"], "breakpoint");
        assert_eq!(v["stop"]["frame"]["line"], 13);
        assert_eq!(v["stop"]["threadId"], 0x1a2b);
        assert!(v["commands"][0]["output"].as_str().unwrap().contains("= 42"));
        assert_eq!(v["detached"], true);
        assert!(v.get("exitedAt").is_none());
        assert_eq!(v["sessionId"], "ses_01J00000000000000000000000");
        let killed = batch_result(&p, &BatchTranscript { exited: true, ..t }, Some("2026-10-11T09:00:00.000Z".into()));
        assert_eq!(killed["exitedAt"], "2026-10-11T09:00:00.000Z");
        let cfg =
            print_config(&p, Path::new("/usr/bin/lldb"), &["-s".into(), "/s".into()], &["process continue".into()]);
        assert_eq!(cfg["lldb"]["env"]["PYTHONHOME"], "/pbs");
        assert_eq!(cfg["broker"]["lldbUrl"], "unix-connect:///r/gdb.sock");
    }
}
