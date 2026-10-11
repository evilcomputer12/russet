// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet debug` (ARCHITECTURE §7.6, §8.7, §17.1; RD §20). Owner: W7 (Wave 1), W18 (`jit-bench`, Wave 3).
//!
//! `russet debug` calls `debug.prepare` (build → sign → install-if-changed → suspended launch → broker
//! pre-attach), appends the `--break` breakpoints and `process continue` to the daemon's Python-free
//! `attach.lldb`, and runs LLDB with the resolver's environment:
//!
//! - interactive: `lldb -s session.lldb`; when LLDB quits, an owned session is stopped;
//! - `--batch --command CMD…`: the script continues to the first stop, runs the commands there, and ends with
//!   `process detach` (`--detach`) or `process kill` [D 84]; `--json` prints `{sessionId, pid, stop{reason,
//!   threadId, frame{function, file, line}}, commands[{command, output}], detached, exitedAt?}`;
//! - `--detach` makes the session `detached` (`debug.prepare {sessionLease: detached}`), so it outlives the CLI
//!   and stays `running` with `debugger: detached` after LLDB's `D`;
//! - `--print-config` prints the LLDB command line, environment and commands instead of running LLDB;
//! - `russet debug prepare --json` prints `debug.prepare`'s result and keeps the (detached) session.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::common::{AttachMode, SessionLease};
use russet_protocol::debug::{
    DebugFrontend, DebugPrepare, DebugPrepareParams, DebugPrepareResult, DebuggerFlavor, JitPolicy, SymbolTier,
};
use russet_protocol::run::{RunStop, RunStopParams};
use russetd::pipeline::lldb::{self, BatchEnd, BatchSpec, BatchTranscript, Breakpoint};

use super::run::{pipeline_args, project_dir, start_params};
use crate::CliCtx;

/// The `debug` command.
pub fn command() -> Command {
    let debug = pipeline_args(Command::new("debug"))
        .about("Build, install, launch and debug with LLDB [--break FILE:LINE]… [--batch] [--detach]; debug prepare, jit-bench")
        .args_conflicts_with_subcommands(true)
        .arg(Arg::new("break").long("break").value_name("FILE:LINE").action(ArgAction::Append).help("A breakpoint"))
        .arg(Arg::new("stop-on-entry").long("stop-on-entry").action(ArgAction::SetTrue).help("Stay stopped after the attach"))
        .arg(Arg::new("attach").long("attach").action(ArgAction::SetTrue).help("Attach to a running app (W11)"))
        .arg(Arg::new("pid").long("pid").value_name("N").requires("attach").help("With --attach: the process id"))
        .arg(Arg::new("bundle").long("bundle").value_name("B").requires("attach").help("With --attach: the bundle id"))
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
        .arg(Arg::new("detach").long("detach").action(ArgAction::SetTrue).help("Leave the app running when LLDB is done"))
        .arg(Arg::new("lldb").long("lldb").value_name("PATH").help("Use this lldb instead of the resolved one"));
    debug
        .subcommand(
            pipeline_args(Command::new("prepare"))
                .about("Run the pipeline up to the broker attach and print the plan (keeps a detached session)")
                .arg(
                    Arg::new("attach-mode")
                        .long("attach-mode")
                        .value_name("MODE")
                        .value_parser(["broker", "legacy-async"]),
                ),
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
    let mut params = prepare_params(m, &project, cx.global.device.clone(), SessionLease::Detached)?;
    params.frontend = Some(DebugFrontend::None);
    let prepared = cx.peer.call::<DebugPrepare>(params).await?;
    let v = serde_json::to_value(&prepared).map_err(|e| Error::new(codes::core::INTERNAL, e.to_string()))?;
    cx.reporter.result(&v);
    Ok(ExitCode::SUCCESS)
}

async fn debug(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    if m.get_flag("attach") {
        return Err(Error::new(codes::core::NOT_IMPLEMENTED, "`russet debug --attach` lands with W11 (debug.attach)")
            .with_hint("Launch and debug the app with `russet debug`."));
    }
    let project = project_dir(cx)?;
    let detach = m.get_flag("detach");
    let lease = if detach { SessionLease::Detached } else { SessionLease::Owned };
    let params = prepare_params(m, &project, cx.global.device.clone(), lease)?;
    let stop_on_entry = params.stop_on_entry.unwrap_or(false);
    let bps = breakpoints(m)?;
    let batch = m.get_flag("batch").then(|| BatchSpec {
        commands: m.get_many::<String>("command").into_iter().flatten().cloned().collect(),
        end: if detach { BatchEnd::Detach } else { BatchEnd::Kill },
    });
    let prepared = cx.peer.call::<DebugPrepare>(params).await?;
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
    cmd.args(&args).envs(&prepared.lldb.env).kill_on_drop(true);
    let outcome = if let Some(batch) = &batch {
        cmd.stdin(std::process::Stdio::null());
        let out = cmd.output().await.map_err(|e| lldb_spawn_error(&program, e))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let t = lldb::parse_batch_output(&text, &batch.commands);
        let exited_at = t.exited.then(russetd::pipeline::logs::now_rfc3339);
        if cx.global.json {
            cx.reporter.result(&batch_result(&prepared, &t, exited_at));
        } else {
            cx.reporter.result(&serde_json::Value::String(text.into_owned()));
        }
        out.status
    } else {
        let mut child = cmd.spawn().map_err(|e| lldb_spawn_error(&program, e))?;
        child.wait().await.map_err(|e| Error::new(codes::core::IO, format!("waiting for lldb: {e}")))?
    };
    if !detach {
        // LLDB detaches on quit after `process connect`; an owned session's app is killed, as without --detach.
        let _ = cx.peer.call::<RunStop>(RunStopParams { session_id: prepared.session_id.clone(), mode: None }).await;
    }
    if !outcome.success() && batch.is_some() {
        return Err(Error::new(codes::debug::LLDB_UNSUPPORTED, format!("lldb exited with {outcome}"))
            .with_hint("Run `russet doctor`, or rerun without --batch to see LLDB's output."));
    }
    Ok(ExitCode::SUCCESS)
}

fn lldb_spawn_error(program: &Path, e: std::io::Error) -> Error {
    Error::new(codes::toolchain::MISSING, format!("cannot run {}: {e}", program.display()))
        .with_hint("Run `russet doctor` to check LLDB, or pass `--lldb PATH`.")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeMap;

    use russet_protocol::debug::{BrokerInfo, BrokerTransport, DebugToolInfo};
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
            component: "lldb".into(),
            version: "21.0.0".into(),
            path: "/usr/lib/swift/usr/bin/lldb".into(),
            provenance: "uv".into(),
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
