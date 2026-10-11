// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet run`, `russet sessions`, `russet stop` (ARCHITECTURE §8.7). Owner: W7 (run, sessions, stop), W11
//! (relaunch, reload).
//!
//! - `russet run` calls `run.start` (build → sign → install-if-changed → launch) and then follows the app's
//!   output until it exits or Ctrl-C (which stops it). `--detach` prints `{sessionId, pid, bundleId}` and leaves a
//!   detached session (§7.6); `--background` returns the job id at once. `--exit-code app` exits with the app's
//!   status (128 + signal for signals).
//! - `russet sessions` lists `SessionSummary` values (`--json`: `{sessions: […]}`).
//! - `russet stop [<session> | --all]` ends sessions; `--detach` leaves the apps running.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::ids::SessionId;
use russet_protocol::common::{LogEntry, SessionLease, SessionSummary};
use russet_protocol::run::{
    InstallMode, LaunchMode, RunList, RunListParams, RunOutputQuery, RunOutputQueryParams, RunStart, RunStartParams,
    RunStop, RunStopParams, StopMode,
};

use crate::CliCtx;

/// This area's top-level commands, as a [`super::group`].
pub fn command() -> Command {
    super::group(
        "run",
        [
            run_command(),
            Command::new("sessions").about("List run, debug and Flutter sessions"),
            Command::new("stop")
                .about("Stop sessions: kill the app, or leave it running with --detach")
                .arg(Arg::new("session").value_name("SESSION").help("A session id from `russet sessions`"))
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .conflicts_with("session")
                        .help("Every live session"),
                )
                .arg(Arg::new("detach").long("detach").action(ArgAction::SetTrue).help("Leave the apps running")),
            super::stub("relaunch", "Quick Relaunch a Swift session"),
            super::stub("reload", "Hot reload a session [--restart]"),
        ],
    )
}

/// Options `russet run` and `russet debug` share.
pub fn pipeline_args(c: Command) -> Command {
    c.arg(
        Arg::new("configuration")
            .long("configuration")
            .value_name("C")
            .help("debug (default), release, profile or a named profile"),
    )
    .arg(Arg::new("no-build").long("no-build").action(ArgAction::SetTrue).help("Use the last build"))
    .arg(
        Arg::new("install")
            .long("install")
            .value_name("WHEN")
            .value_parser(["if-changed", "always", "never"])
            .help("Install when the signed app changed (default), always, or never"),
    )
    .arg(
        Arg::new("launch")
            .long("launch")
            .value_name("HOW")
            .value_parser(["auto", "coredevice", "dvt"])
            .help("Launch through CoreDevice (default: auto) or DVT"),
    )
    .arg(Arg::new("env").long("env").value_name("K=V").action(ArgAction::Append).help("Environment for the app"))
    .arg(Arg::new("app-args").num_args(0..).last(true).value_name("APP ARGS").help("Arguments for the app"))
}

fn run_command() -> Command {
    pipeline_args(Command::new("run").about("Build, install and launch the app, then follow its output"))
        .arg(Arg::new("launch-none").long("no-launch").action(ArgAction::SetTrue).help("Build, sign and install only"))
        .arg(
            Arg::new("detach")
                .long("detach")
                .action(ArgAction::SetTrue)
                .conflicts_with("background")
                .help("Wait for the launch, print {sessionId, pid, bundleId}, leave the session running"),
        )
        .arg(Arg::new("background").long("background").action(ArgAction::SetTrue).help("Return the job id at once"))
        .arg(
            Arg::new("exit-code")
                .long("exit-code")
                .value_name("SOURCE")
                .value_parser(["app"])
                .help("`app`: exit with the app's status (128 + signal)"),
        )
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    match m.subcommand() {
        Some(("run", sub)) => run_app(sub, cx).await,
        Some(("sessions", _)) => sessions(cx).await,
        Some(("stop", sub)) => stop(sub, cx).await,
        _ => super::not_implemented(m),
    }
}

/// The project root: `--project`, else the current directory (absolute; ARCHITECTURE §7.3).
pub fn project_dir(cx: &CliCtx) -> Result<PathBuf> {
    let dir = match &cx.global.project {
        Some(p) => p.clone(),
        None => {
            std::env::current_dir().map_err(|e| Error::new(codes::core::IO, format!("no current directory: {e}")))?
        }
    };
    std::fs::canonicalize(&dir).map_err(|_| {
        Error::new(codes::config::PROJECT_NOT_FOUND, format!("no project at `{}`", dir.display()))
            .with_hint("Run inside the project, or pass `--project DIR`.")
    })
}

/// `--env K=V`… into a map.
pub fn env_map(m: &ArgMatches) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for kv in m.get_many::<String>("env").into_iter().flatten() {
        let (k, v) = kv.split_once('=').ok_or_else(|| {
            Error::new(codes::core::INVALID_ARGUMENT, format!("`--env {kv}` is not K=V"))
                .with_hint("Write `--env NAME=value`.")
        })?;
        out.insert(k.to_owned(), v.to_owned());
    }
    Ok(out)
}

/// The `run.start` params of `russet run` and `russet debug`.
pub fn start_params(
    m: &ArgMatches,
    project: &std::path::Path,
    device: Option<String>,
    lease: SessionLease,
) -> Result<RunStartParams> {
    let args: Vec<String> = m.get_many::<String>("app-args").into_iter().flatten().cloned().collect();
    let env = env_map(m)?;
    let install = m.get_one::<String>("install").map(|s| match s.as_str() {
        "always" => InstallMode::Always,
        "never" => InstallMode::Never,
        _ => InstallMode::IfChanged,
    });
    let mut launch = m.get_one::<String>("launch").map(|s| match s.as_str() {
        "coredevice" => LaunchMode::Coredevice,
        "dvt" => LaunchMode::Dvt,
        _ => LaunchMode::Auto,
    });
    if m.try_get_one::<bool>("launch-none").ok().flatten().copied().unwrap_or(false) {
        launch = Some(LaunchMode::None);
    }
    Ok(RunStartParams {
        project: project.to_string_lossy().into_owned(),
        target: None,
        configuration: m.get_one::<String>("configuration").cloned(),
        udid: device,
        args: (!args.is_empty()).then_some(args),
        env: (!env.is_empty()).then_some(env),
        build: m.get_flag("no-build").then_some(false),
        install,
        launch,
        session_lease: Some(lease),
        background: m.try_get_one::<bool>("background").ok().flatten().copied().filter(|b| *b),
    })
}

fn json(v: &impl serde::Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| Error::new(codes::core::INTERNAL, format!("encoding the result: {e}")))
}

async fn run_app(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let project = project_dir(cx)?;
    let detach = m.get_flag("detach") || m.get_flag("background");
    let lease = if detach { SessionLease::Detached } else { SessionLease::Owned };
    let params = start_params(m, &project, cx.global.device.clone(), lease)?;
    let started = cx.peer.call::<RunStart>(params).await?;
    let Some(session_id) = started.session_id.clone() else {
        // --no-launch: built, signed, installed.
        cx.reporter.result(&json(&started)?);
        return Ok(ExitCode::SUCCESS);
    };
    if detach {
        cx.reporter.result(&serde_json::json!({
            "sessionId": session_id, "pid": started.pid, "bundleId": started.bundle_id,
        }));
        return Ok(ExitCode::SUCCESS);
    }
    let summary = follow(cx, &session_id).await?;
    let want_app_status = m.get_one::<String>("exit-code").is_some_and(|s| s == "app");
    Ok(if want_app_status { app_exit_code(summary.as_ref()) } else { ExitCode::SUCCESS })
}

/// Prints the session's output until it ends (or Ctrl-C, which stops it); returns the final summary.
pub async fn follow(cx: &CliCtx, session_id: &SessionId) -> Result<Option<SessionSummary>> {
    let mut cursor = OutputCursor::default();
    loop {
        let q = RunOutputQueryParams {
            session_id: session_id.clone(),
            since_ms: cursor.since_ms,
            limit: None,
            filter: None,
        };
        let out = cx.peer.call::<RunOutputQuery>(q).await?;
        for e in cursor.fresh(out.entries) {
            cx.reporter.result(&json(&e)?);
        }
        let list = cx.peer.call::<RunList>(RunListParams {}).await?;
        let me = list.sessions.into_iter().find(|s| &s.session_id == session_id);
        match me {
            Some(s) if s.ended_at.is_none() => {}
            other => {
                // The app may have printed its last lines between the query and the list.
                let q = RunOutputQueryParams {
                    session_id: session_id.clone(),
                    since_ms: cursor.since_ms,
                    limit: None,
                    filter: None,
                };
                if let Ok(out) = cx.peer.call::<RunOutputQuery>(q).await {
                    for e in cursor.fresh(out.entries) {
                        cx.reporter.result(&json(&e)?);
                    }
                }
                return Ok(other);
            }
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                let _ = cx.peer.call::<RunStop>(RunStopParams { session_id: session_id.clone(), mode: None }).await;
                return Ok(None);
            }
            () = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    }
}

/// Polls `run.output.query` without repeating entries: `sinceMs` is inclusive, so the entries already printed at
/// the last millisecond are skipped.
#[derive(Debug, Default)]
pub struct OutputCursor {
    pub since_ms: Option<i64>,
    printed_at_since: usize,
}

impl OutputCursor {
    /// The entries not printed yet, in order.
    pub fn fresh(&mut self, entries: Vec<LogEntry>) -> Vec<LogEntry> {
        let mut out = Vec::new();
        let mut skip = self.printed_at_since;
        for e in entries {
            let ms = ts_ms(&e.ts);
            if let (Some(since), Some(ms)) = (self.since_ms, ms) {
                if ms < since {
                    continue;
                }
                if ms == since && skip > 0 {
                    skip -= 1;
                    continue;
                }
            }
            match (ms, self.since_ms) {
                (Some(ms), Some(since)) if ms == since => self.printed_at_since += 1,
                (Some(ms), _) => {
                    self.since_ms = Some(ms);
                    self.printed_at_since = 1;
                }
                (None, _) => {}
            }
            out.push(e);
        }
        out
    }
}

fn ts_ms(ts: &str) -> Option<i64> {
    russetd::pipeline::logs::parse_ts_ms(ts)
}

/// `--exit-code app`: the app's status, 128 + signal for a signal (RD §20).
pub fn app_exit_code(s: Option<&SessionSummary>) -> ExitCode {
    let exit = s.and_then(|s| s.exit.as_ref());
    match exit {
        Some(e) if e.signal.is_some() => ExitCode::from((128 + e.signal.unwrap_or(0)).clamp(0, 255) as u8),
        Some(e) => ExitCode::from(e.code.unwrap_or(0).clamp(0, 255) as u8),
        None => ExitCode::SUCCESS,
    }
}

async fn sessions(cx: &CliCtx) -> Result<ExitCode> {
    let list = cx.peer.call::<RunList>(RunListParams {}).await?;
    cx.reporter.result(&json(&list)?);
    Ok(ExitCode::SUCCESS)
}

async fn stop(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let mode = if m.get_flag("detach") { StopMode::Detach } else { StopMode::Kill };
    let ids: Vec<SessionId> = if m.get_flag("all") {
        let list = cx.peer.call::<RunList>(RunListParams {}).await?;
        list.sessions.into_iter().filter(|s| s.ended_at.is_none()).map(|s| s.session_id).collect()
    } else {
        let Some(id) = m.get_one::<String>("session") else {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "name a session or pass --all")
                .with_hint("List them with `russet sessions`."));
        };
        vec![
            serde_json::from_value(serde_json::Value::String(id.clone()))
                .map_err(|e| Error::new(codes::core::INVALID_ARGUMENT, format!("bad session id: {e}")))?,
        ]
    };
    for id in &ids {
        cx.peer.call::<RunStop>(RunStopParams { session_id: id.clone(), mode: Some(mode) }).await?;
    }
    cx.reporter.result(&serde_json::json!({ "stopped": ids }));
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use russet_protocol::common::{LogLevel, LogSource, SessionExit};

    use super::*;

    fn parse(args: &[&str]) -> ArgMatches {
        let root = crate::root_command();
        root.try_get_matches_from(args).unwrap()
    }

    fn sub<'a>(m: &'a ArgMatches, name: &str) -> &'a ArgMatches {
        let (n, s) = m.subcommand().unwrap();
        assert_eq!(n, name);
        s
    }

    #[test]
    fn run_flags_become_run_start_params() {
        let m = parse(&[
            "russet",
            "run",
            "--configuration",
            "release",
            "--no-build",
            "--install",
            "always",
            "--launch",
            "dvt",
            "--env",
            "A=1",
            "--env",
            "B=x=y",
            "--",
            "-RussetProbeCrash",
            "1",
        ]);
        let p =
            start_params(sub(&m, "run"), std::path::Path::new("/p"), Some("dev1".into()), SessionLease::Owned).unwrap();
        assert_eq!(p.project, "/p");
        assert_eq!(p.configuration.as_deref(), Some("release"));
        assert_eq!(p.build, Some(false));
        assert_eq!(p.install, Some(InstallMode::Always));
        assert_eq!(p.launch, Some(LaunchMode::Dvt));
        assert_eq!(p.udid.as_deref(), Some("dev1"));
        assert_eq!(p.args, Some(vec!["-RussetProbeCrash".to_owned(), "1".to_owned()]));
        let env = p.env.unwrap();
        assert_eq!((env["A"].as_str(), env["B"].as_str()), ("1", "x=y"));
        let m = parse(&["russet", "run", "--no-launch"]);
        let p = start_params(sub(&m, "run"), std::path::Path::new("/p"), None, SessionLease::Detached).unwrap();
        assert_eq!(p.launch, Some(LaunchMode::None));
        assert_eq!(p.build, None);
        assert_eq!(p.session_lease, Some(SessionLease::Detached));
        let m = parse(&["russet", "run", "--env", "NOPE"]);
        assert_eq!(env_map(sub(&m, "run")).unwrap_err().code(), codes::core::INVALID_ARGUMENT);
        assert!(crate::root_command().try_get_matches_from(["russet", "run", "--detach", "--background"]).is_err());
    }

    #[test]
    fn stop_and_sessions_parse() {
        let m = parse(&["russet", "stop", "--all", "--detach"]);
        let s = sub(&m, "stop");
        assert!(s.get_flag("all") && s.get_flag("detach"));
        let m = parse(&["russet", "--json", "sessions"]);
        assert!(m.get_flag("json"));
        assert!(crate::root_command().try_get_matches_from(["russet", "stop", "ses_x", "--all"]).is_err());
    }

    #[test]
    fn the_output_cursor_never_repeats_an_entry() {
        let e = |ts: &str, msg: &str| LogEntry {
            ts: ts.into(),
            source: LogSource::Stdout,
            level: LogLevel::Default,
            message: msg.into(),
            pid: None,
            tid: None,
            process: None,
            subsystem: None,
            category: None,
            image_uuid: None,
            image_offset: None,
            mach_ts: None,
        };
        let mut c = OutputCursor::default();
        let first = c.fresh(vec![e("2026-10-11T09:00:00.100Z", "a"), e("2026-10-11T09:00:00.200Z", "b")]);
        assert_eq!(first.len(), 2);
        assert_eq!(c.since_ms, Some(1_791_709_200_200));
        // The server returns everything at or after 200 ms again, plus new ones.
        let again = c.fresh(vec![
            e("2026-10-11T09:00:00.200Z", "b"),
            e("2026-10-11T09:00:00.200Z", "c"),
            e("2026-10-11T09:00:00.300Z", "d"),
        ]);
        let msgs: Vec<&str> = again.iter().map(|x| x.message.as_str()).collect();
        assert_eq!(msgs, ["c", "d"]);
        assert!(c.fresh(vec![e("2026-10-11T09:00:00.300Z", "d")]).is_empty());
    }

    #[test]
    fn app_exit_codes() {
        let s = |code: Option<i32>, signal: Option<i32>| SessionSummary {
            session_id: serde_json::from_value(serde_json::json!("ses_01J00000000000000000000000")).unwrap(),
            kind: russet_protocol::common::SessionKind::Run,
            bundle_id: "b".into(),
            udid: "u".into(),
            pid: None,
            project: None,
            state: russet_protocol::common::SessionState::Exited,
            lease: SessionLease::Owned,
            debugger: russet_protocol::common::DebuggerState::None,
            attach_mode: None,
            started_at: "t".into(),
            ended_at: Some("t".into()),
            exit: Some(SessionExit { code, signal, crash_id: None }),
        };
        assert_eq!(app_exit_code(Some(&s(Some(3), None))), ExitCode::from(3));
        assert_eq!(app_exit_code(Some(&s(None, Some(11)))), ExitCode::from(139));
        assert_eq!(app_exit_code(None), ExitCode::SUCCESS);
    }
}
