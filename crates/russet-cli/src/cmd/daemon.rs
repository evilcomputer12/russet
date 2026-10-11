// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet daemon run|start|stop|restart|status|logs|path` (ARCHITECTURE §8.7). Owner: W2.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result, codes};
use russet_ipc::Endpoint;
use russet_protocol::daemon::{
    DaemonLogBatch, DaemonLogsSubscribe, DaemonLogsSubscribeParams, DaemonShutdown, DaemonShutdownParams, DaemonStatus,
    DaemonStatusParams, DaemonStatusResult, ShutdownReason, TraceLevel,
};
use serde::Serialize;

use crate::CliCtx;

/// How long `stop` waits for the daemon to exit.
const STOP_WAIT: Duration = Duration::from_secs(15);

/// The `daemon` command.
pub fn command() -> Command {
    let idle = || {
        Arg::new("idle-timeout")
            .long("idle-timeout")
            .value_name("DURATION")
            .help("Exit after this long without clients, jobs or detached sessions (30m; 0 = never)")
    };
    Command::new("daemon")
        .about("The per-user daemon: run, start, stop, restart, status, logs, path")
        .subcommand_required(true)
        .subcommand(
            Command::new("run")
                .about("Run the daemon in this process (what autostart does, detached)")
                .arg(
                    Arg::new("detach")
                        .long("detach")
                        .action(ArgAction::SetTrue)
                        .help("Start a new session; log only to files"),
                )
                .arg(Arg::new("foreground").long("foreground").action(ArgAction::SetTrue).help("Also log to stderr"))
                .arg(idle())
                .arg(
                    Arg::new("allow-live-device")
                        .long("allow-live-device")
                        .action(ArgAction::SetTrue)
                        .help("Let a development build use the live device backend (lane A tests only)"),
                ),
        )
        .subcommand(Command::new("start").about("Start the daemon unless it already runs").arg(idle()))
        .subcommand(Command::new("stop").about("Stop the daemon").arg(
            Arg::new("force").long("force").action(ArgAction::SetTrue).help("Stop even with active jobs and sessions"),
        ))
        .subcommand(Command::new("restart").about("Stop and start the daemon").arg(
            Arg::new("force").long("force").action(ArgAction::SetTrue).help("Stop even with active jobs and sessions"),
        ))
        .subcommand(Command::new("status").about("The daemon's state: build, connections, jobs, leases"))
        .subcommand(
            Command::new("logs")
                .about("The daemon's log (state/logs); --follow streams it")
                .arg(Arg::new("follow").short('f').long("follow").action(ArgAction::SetTrue).help("Stream new records"))
                .arg(
                    Arg::new("level")
                        .long("level")
                        .value_name("LEVEL")
                        .value_parser(["trace", "debug", "info", "warn", "error"])
                        .help("The lowest level to stream (info)"),
                )
                .arg(
                    Arg::new("lines")
                        .short('n')
                        .long("lines")
                        .value_name("N")
                        .value_parser(clap::value_parser!(usize))
                        .help("How many lines of the current file to print (50)"),
                ),
        )
        .subcommand(Command::new("path").about("Russet's directories, the daemon's endpoint and files"))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some((_, m)) = m.subcommand() else { return Err(Error::invalid_argument("no daemon command")) };
    match m.subcommand() {
        Some(("run", a)) => run_daemon(a).await,
        Some(("start", a)) => start(a, cx).await,
        Some(("stop", a)) => stop(a.get_flag("force"), cx).await,
        Some(("restart", a)) => {
            stop(a.get_flag("force"), cx).await?;
            start_with(None, cx).await
        }
        Some(("status", _)) => status(cx).await,
        Some(("logs", a)) => logs(a, cx).await,
        Some(("path", _)) => path(cx),
        _ => Err(Error::invalid_argument("unknown daemon command").with_hint("Run `russet daemon --help`.")),
    }
}

fn idle_timeout(a: &ArgMatches) -> Result<Option<Duration>> {
    match a.get_one::<String>("idle-timeout") {
        None => Ok(None),
        Some(s) => russet_core::time::parse_duration(s).map(Some).ok_or_else(|| {
            Error::invalid_argument(format!("`{s}` is not a duration")).with_hint("Use 30m, 90s, 500ms or 0.")
        }),
    }
}

async fn run_daemon(a: &ArgMatches) -> Result<ExitCode> {
    let dirs = RussetDirs::from_env()?;
    let mut o = russetd::DaemonOptions::from_env(dirs)?;
    o.detach = a.get_flag("detach");
    o.foreground = a.get_flag("foreground") || !o.detach;
    o.allow_live_device = a.get_flag("allow-live-device");
    if let Some(t) = idle_timeout(a)? {
        o.idle_timeout = (!t.is_zero()).then_some(t);
    }
    russetd::run(o).await?;
    Ok(ExitCode::SUCCESS)
}

/// The human form of `daemon.status`.
pub fn human_status(s: &DaemonStatusResult) -> String {
    let mut lines = vec![
        format!("russetd {} (build {}), pid {}", s.version, s.build_id, s.pid),
        format!("endpoint: {}", s.endpoint),
        format!("up for {}s, {} RSS", s.uptime_ms / 1000, russet_core::fsx::human_bytes(s.rss_bytes)),
        format!("device backend: {}", s.device_backend),
        format!("jobs: {} running, {} queued", s.jobs.running, s.jobs.queued),
    ];
    if let Some(web) = &s.web {
        lines.push(format!("web: {web}"));
    }
    lines.push(format!("connections: {}", s.connections.len()));
    for c in &s.connections {
        lines.push(format!("  {} {:?} {}", c.id, c.kind, c.name).to_lowercase());
    }
    lines.push(format!("leases: {}", s.leases.len()));
    lines.join("\n")
}

async fn start(a: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    start_with(idle_timeout(a)?, cx).await
}

async fn start_with(idle: Option<Duration>, cx: &CliCtx) -> Result<ExitCode> {
    let dirs = RussetDirs::from_env()?;
    let (peer, _) = russet_ipc::connect_or_spawn(&dirs, cx.peer.client_info(), crate::spawn::spawn_spec(idle)?).await?;
    let status = peer.call::<DaemonStatus>(DaemonStatusParams {}).await?;
    peer.close();
    cx.output(&status, human_status)?;
    Ok(ExitCode::SUCCESS)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Stopped {
    stopped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
}

async fn stop(force: bool, cx: &CliCtx) -> Result<ExitCode> {
    let peer = match cx.peer.existing().await {
        Ok(p) => p,
        Err(e) if e.code() == codes::ipc::DAEMON_UNAVAILABLE => {
            cx.output(&Stopped { stopped: false, pid: None }, |_| "the daemon is not running".to_owned())?;
            return Ok(ExitCode::SUCCESS);
        }
        Err(e) => return Err(e),
    };
    let pid = cx.peer.initialize_result().map(|r| r.server.pid);
    let r = peer.call::<DaemonShutdown>(DaemonShutdownParams { force, reason: Some(ShutdownReason::Request) }).await?;
    if !r.accepted {
        return Err(Error::new(
            codes::core::BUSY,
            format!("the daemon is busy: {} active jobs, sessions or leases", r.active_sessions),
        )
        .with_hint("Let them finish, or stop it anyway with `russet daemon stop --force`."));
    }
    let endpoint = Endpoint::from_env(&RussetDirs::from_env()?)?;
    let deadline = Instant::now() + STOP_WAIT;
    loop {
        match endpoint.open(Duration::from_millis(200)).await {
            Err(e) if e.code() == codes::ipc::DAEMON_UNAVAILABLE => break,
            Err(e) => return Err(e),
            Ok(p) => {
                p.close();
                if Instant::now() >= deadline {
                    return Err(Error::new(codes::core::TIMEOUT, "the daemon did not exit in time")
                        .with_hint("Check `russet daemon logs`; `russet daemon stop --force` skips the job grace."));
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    cx.output(&Stopped { stopped: true, pid }, |s| match s.pid {
        Some(pid) => format!("stopped the daemon (pid {pid})"),
        None => "stopped the daemon".to_owned(),
    })?;
    Ok(ExitCode::SUCCESS)
}

async fn status(cx: &CliCtx) -> Result<ExitCode> {
    let s = cx.peer.call::<DaemonStatus>(DaemonStatusParams {}).await?;
    cx.output(&s, human_status)?;
    Ok(ExitCode::SUCCESS)
}

/// The newest `russetd.YYYY-MM-DD.log` in `dir`.
fn newest_log(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("russetd.") && n.ends_with(".log"))
        })
        .collect();
    files.sort();
    files.pop()
}

async fn logs(a: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let level = match a.get_one::<String>("level").map(String::as_str) {
        Some("trace") => TraceLevel::Trace,
        Some("debug") => TraceLevel::Debug,
        Some("warn") => TraceLevel::Warn,
        Some("error") => TraceLevel::Error,
        _ => TraceLevel::Info,
    };
    if !a.get_flag("follow") {
        let dirs = RussetDirs::from_env()?;
        let lines = a.get_one::<usize>("lines").copied().unwrap_or(50);
        let Some(file) = newest_log(&dirs.logs()) else {
            cx.output(&serde_json::json!({"lines": []}), |_| format!("no daemon log in {}", dirs.logs().display()))?;
            return Ok(ExitCode::SUCCESS);
        };
        let text = std::fs::read_to_string(&file).map_err(|e| Error::io(format!("reading {}", file.display()), e))?;
        let all: Vec<String> = text.lines().map(|l| russet_core::redact::redact_text(l).into_owned()).collect();
        let tail: Vec<String> = all[all.len().saturating_sub(lines)..].to_vec();
        cx.output(&serde_json::json!({"file": file.display().to_string(), "lines": tail}), |_| tail.join("\n"))?;
        return Ok(ExitCode::SUCCESS);
    }
    let peer = cx.peer.get().await?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    peer.on_notification::<DaemonLogBatch, _>(move |batch| {
        let _ = tx.send(batch);
    });
    peer.call::<DaemonLogsSubscribe>(DaemonLogsSubscribeParams { min_level: Some(level) }).await?;
    loop {
        tokio::select! {
            batch = rx.recv() => {
                let Some(batch) = batch else { break };
                if let Some(n) = batch.dropped {
                    cx.reporter.human(&format!("({n} records dropped)"));
                }
                for r in batch.records {
                    let line = russetd::daemon::logging::format_text(&r);
                    cx.reporter.item(&serde_json::to_value(&r).unwrap_or_default(), &line);
                }
            }
            _ = tokio::signal::ctrl_c() => break,
            () = peer.closed() => break,
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Paths {
    config: String,
    data: String,
    state: String,
    cache: String,
    runtime: String,
    endpoint: String,
    discovery: String,
    logs: String,
    audit: String,
}

fn path(cx: &CliCtx) -> Result<ExitCode> {
    let dirs = RussetDirs::from_env()?;
    let endpoint =
        if crate::wants_in_process(&cx.global) { "inproc".to_owned() } else { Endpoint::from_env(&dirs)?.to_string() };
    let show = |p: &std::path::Path| p.display().to_string();
    let paths = Paths {
        config: show(&dirs.config),
        data: show(&dirs.data),
        state: show(&dirs.state),
        cache: show(&dirs.cache),
        runtime: show(&dirs.runtime),
        endpoint,
        discovery: show(&dirs.daemon_json()),
        logs: show(&dirs.logs()),
        audit: show(&dirs.audit_log()),
    };
    cx.output(&paths, |p| {
        format!(
            "config:    {}\ndata:      {}\nstate:     {}\ncache:     {}\nruntime:   {}\nendpoint:  {}\ndiscovery: {}\nlogs:      {}\naudit:     {}",
            p.config, p.data, p.state, p.cache, p.runtime, p.endpoint, p.discovery, p.logs, p.audit
        )
    })?;
    Ok(ExitCode::SUCCESS)
}
