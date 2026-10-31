// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet logs` (ARCHITECTURE §8.7). Owner: W4 (`--session` reads W7's run.output.query).
//!
//! `russet logs [--app B | --session S | --pid N] [--level L] [--grep TEXT] [--follow] [--duration D]`.
//! Device logs come from `logs.tail` (the device's tail ring) and are followed with its cursor; `--session` reads
//! `run.output.query` and then follows for `--duration`. With `--json` the output is NDJSON, one `LogEntry`
//! (ARCHITECTURE §9.5) per line, which the Wave-1 exit script counts with `grep -c`.

use std::collections::HashSet;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::Method;
use russet_protocol::common::{LogEntry, LogLevel};
use russet_protocol::logs as p;

use crate::CliCtx;

/// How often `--follow` polls.
const POLL: Duration = Duration::from_millis(400);

/// The `logs` command.
pub fn command() -> Command {
    Command::new("logs")
        .about("Device and session logs [--app B | --session S | --pid N] [--level L] [--grep TEXT] [--follow]")
        .arg(Arg::new("app").long("app").value_name("BUNDLE_ID").help("Logs of an app"))
        .arg(Arg::new("session").long("session").value_name("SESSION_ID").help("Output of a run or debug session"))
        .arg(
            Arg::new("pid")
                .long("pid")
                .value_name("PID")
                .value_parser(clap::value_parser!(u32))
                .help("Logs of a process"),
        )
        .group(ArgGroup::new("what").args(["app", "session", "pid"]))
        .arg(
            Arg::new("level")
                .long("level")
                .value_name("LEVEL")
                .help("Minimum level: debug, info, default, error, fault"),
        )
        .arg(Arg::new("grep").long("grep").value_name("TEXT").help("Only messages containing TEXT (case-insensitive)"))
        .arg(
            Arg::new("lines")
                .long("lines")
                .value_name("N")
                .value_parser(clap::value_parser!(u32))
                .help("Backlog lines (default 200)"),
        )
        .arg(Arg::new("follow").short('f').long("follow").action(ArgAction::SetTrue).help("Keep printing new entries"))
        .arg(
            Arg::new("duration")
                .long("duration")
                .value_name("DURATION")
                .help("Follow this long (10s, 2m; implies --follow)"),
        )
}

/// `debug | info | default | error | fault` (Apple's names; `notice` is accepted as `default`).
pub fn parse_level(s: &str) -> Result<LogLevel> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "debug" => LogLevel::Debug,
        "info" => LogLevel::Info,
        "default" | "notice" => LogLevel::Default,
        "error" => LogLevel::Error,
        "fault" => LogLevel::Fault,
        other => {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, format!("unknown log level {other:?}"))
                .with_hint("Use debug, info, default, error or fault."));
        }
    })
}

/// `10s`, `2m`, `500ms`, `1h` or plain seconds.
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().map_err(|_| Error::new(codes::core::INVALID_ARGUMENT, format!("bad duration {s:?}")))?;
    let secs = match unit.trim() {
        "" | "s" => n,
        "ms" => n / 1000.0,
        "m" => n * 60.0,
        "h" => n * 3600.0,
        _ => return Err(Error::new(codes::core::INVALID_ARGUMENT, format!("bad duration {s:?}"))),
    };
    Ok(Duration::from_secs_f64(secs.max(0.0)))
}

/// One human line: `09:00:00.123 default RussetProbe[932] message`.
pub fn human_line(e: &LogEntry) -> String {
    let time = e.ts.split('T').nth(1).map(|t| t.trim_end_matches('Z')).unwrap_or(&e.ts);
    let level = serde_json::to_value(e.level).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
    let who = match (&e.process, e.pid) {
        (Some(p), Some(pid)) => format!("{p}[{pid}]"),
        (Some(p), None) => p.clone(),
        (None, Some(pid)) => format!("[{pid}]"),
        (None, None) => String::new(),
    };
    format!("{time} {level:<7} {who} {}", e.message)
}

fn emit(cx: &CliCtx, e: &LogEntry) -> Result<()> {
    let v = if cx.global.json {
        serde_json::to_value(e)
            .map_err(|err| Error::new(codes::core::INTERNAL, format!("cannot render a log entry: {err}")))?
    } else {
        serde_json::Value::String(human_line(e))
    };
    cx.reporter.result(&v);
    Ok(())
}

/// The `logs.tail` params of a command line.
pub fn tail_params(m: &ArgMatches, udid: Option<String>) -> Result<p::LogsTailParams> {
    Ok(p::LogsTailParams {
        udid,
        bundle_id: m.get_one::<String>("app").cloned(),
        pid: m.get_one::<u32>("pid").copied(),
        lines: m.get_one::<u32>("lines").copied(),
        min_level: m.get_one::<String>("level").map(|l| parse_level(l)).transpose()?,
        contains: m.get_one::<String>("grep").cloned(),
        since: None,
        cursor: None,
    })
}

fn deadline(m: &ArgMatches) -> Result<(bool, Option<Instant>)> {
    let duration = m.get_one::<String>("duration").map(|d| parse_duration(d)).transpose()?;
    let follow = m.get_flag("follow") || duration.is_some();
    Ok((follow, duration.map(|d| Instant::now() + d)))
}

async fn device_logs(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let (follow, until) = deadline(m)?;
    let mut params = tail_params(m, cx.global.device.clone())?;
    loop {
        let r = cx.peer.call::<p::LogsTail>(params.clone()).await?;
        for e in &r.entries {
            emit(cx, e)?;
        }
        if r.cursor.is_some() {
            params.cursor = r.cursor;
        }
        params.lines = Some(2000);
        if !follow || until.is_some_and(|u| Instant::now() >= u) {
            return Ok(ExitCode::SUCCESS);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// `--session`: W7's `run.output.query`, then follow for `--duration`. The params and result go through JSON, so
/// this works with whatever DTO W7's `run.rs` declares for `{sessionId, sinceMs?, limit?, filter?}` →
/// `{entries: LogEntry[], dropped}`.
async fn session_logs(m: &ArgMatches, cx: &CliCtx, session: &str) -> Result<ExitCode> {
    use russet_protocol::run::RunOutputQuery;
    let (follow, until) = deadline(m)?;
    let min_level = m.get_one::<String>("level").map(|l| parse_level(l)).transpose()?;
    let grep = m.get_one::<String>("grep").map(|g| g.to_lowercase());
    let started = Instant::now();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut since_ms: Option<u64> = None;
    loop {
        let mut q =
            serde_json::json!({ "sessionId": session, "limit": m.get_one::<u32>("lines").copied().unwrap_or(2000) });
        if let Some(s) = since_ms {
            q["sinceMs"] = s.into();
        }
        let params: <RunOutputQuery as Method>::Params = serde_json::from_value(q)
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("run.output.query params: {e}")))?;
        let result = cx.peer.call::<RunOutputQuery>(params).await?;
        let value = serde_json::to_value(result)
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("run.output.query result: {e}")))?;
        let entries: Vec<LogEntry> =
            value.get("entries").cloned().map(serde_json::from_value).transpose().ok().flatten().unwrap_or_default();
        for e in entries {
            if min_level.is_some_and(|l| e.level < l)
                || grep.as_ref().is_some_and(|g| !e.message.to_lowercase().contains(g))
            {
                continue;
            }
            if seen.insert((e.ts.clone(), e.message.clone())) {
                emit(cx, &e)?;
            }
        }
        if !follow || until.is_some_and(|u| Instant::now() >= u) {
            return Ok(ExitCode::SUCCESS);
        }
        // Overlap one poll interval; duplicates are dropped by (ts, message).
        since_ms = Some(started.elapsed().saturating_sub(POLL * 2).as_millis() as u64);
        tokio::time::sleep(POLL).await;
    }
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("logs", lm)) = m.subcommand() else {
        return super::not_implemented(m);
    };
    match lm.get_one::<String>("session") {
        Some(s) => session_logs(lm, cx, s).await,
        None => device_logs(lm, cx).await,
    }
}

#[cfg(test)]
mod tests {
    use russet_protocol::common::LogSource;

    use super::*;

    #[test]
    fn levels_and_durations_parse() {
        assert_eq!(parse_level("notice").expect("alias"), LogLevel::Default);
        assert_eq!(parse_level("FAULT").expect("case"), LogLevel::Fault);
        assert!(parse_level("verbose").is_err());
        assert_eq!(parse_duration("10s").expect("s"), Duration::from_secs(10));
        assert_eq!(parse_duration("2m").expect("m"), Duration::from_secs(120));
        assert_eq!(parse_duration("250ms").expect("ms"), Duration::from_millis(250));
        assert!(parse_duration("soon").is_err());
    }

    #[test]
    fn command_lines_become_tail_params() {
        let m = crate::root_command()
            .try_get_matches_from([
                "russet",
                "logs",
                "--app",
                "dev.russet.test.probe",
                "--level",
                "error",
                "--grep",
                "boom",
            ])
            .expect("valid");
        let (_, lm) = m.subcommand().expect("logs");
        let params = tail_params(lm, None).expect("params");
        assert_eq!(params.bundle_id.as_deref(), Some("dev.russet.test.probe"));
        assert_eq!(params.min_level, Some(LogLevel::Error));
        assert_eq!(params.contains.as_deref(), Some("boom"));
        assert!(crate::root_command().try_get_matches_from(["russet", "logs", "--app", "a", "--pid", "1"]).is_err());
    }

    #[test]
    fn human_lines_are_compact() {
        let e = LogEntry {
            ts: "2026-10-11T09:00:00.123Z".into(),
            source: LogSource::Oslog,
            level: LogLevel::Default,
            message: "hello".into(),
            pid: Some(932),
            tid: None,
            process: Some("RussetProbe".into()),
            subsystem: None,
            category: None,
            image_uuid: None,
            image_offset: None,
            mach_ts: None,
        };
        assert_eq!(human_line(&e), "09:00:00.123 default RussetProbe[932] hello");
    }
}
