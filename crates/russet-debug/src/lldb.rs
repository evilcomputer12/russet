// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! LLDB command plans (RD-06; RD §8.4, §9.1–§9.2; ARCHITECTURE §8.7, §16.2, §17.1).
//!
//! - [`attach_commands`]: the Python-free attach sequence. With the broker, `process connect` to a stub that
//!   already owns a stopped process returns at once, even in LLDB's synchronous batch mode (RD V3–V4). The
//!   connect names the `gdb-remote` plugin explicitly: on a host target LLDB otherwise picks another process
//!   plugin and the connect fails silently (verified with Swift 6.4's LLDB 21.0.0, 2026-10-11).
//! - [`legacy_async_commands`]: the M1 fallback (`attachMode: legacy-async`, [D 38]). It is the only plan with
//!   `script` lines, because LLDB's command interpreter cannot switch to asynchronous mode without the script
//!   interpreter, and a synchronous `process connect` to a process-less debugserver never returns (RD V3).
//! - [`session_script`]: what `russet debug` runs: the attach plan, the `--break` breakpoints, `process continue`,
//!   and, for `--batch`, the stop probe, the `--command`s and `process detach` (`--detach`) or `process kill`
//!   [D 84]. Batch scripts are run through `command source -e false`, so a failing `--command` still reaches
//!   the final detach or kill.
//! - [`parse_batch_output`]: turns LLDB's echoed transcript into the `--json` result of ARCHITECTURE §8.7.
//! - [`run_batch`] and [`batch_failure`]: a batch run with a time limit and Ctrl-C handling, and why a run that
//!   never stopped failed (LLDB's batch mode exits 0 even when `process connect` fails; its errors go to stderr).

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The LLDB platform of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LldbPlatform {
    /// `platform select remote-ios`, with `--sysroot` when a local device-symbol cache exists (RD V7).
    RemoteIos { sysroot: Option<PathBuf> },
    /// The host platform (host tests against lldb-server).
    Host,
}

/// What the attach commands need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachPlan {
    pub platform: LldbPlatform,
    /// `--arch` for `target create`, for example `arm64-apple-ios`; `None` lets LLDB read the file.
    pub arch: Option<String>,
    /// The local executable with debug info: the unsigned build product or the signed copy (same UUID).
    pub local_executable: PathBuf,
    /// The app bundle on the device (`/private/var/containers/Bundle/Application/<UUID>/X.app`).
    pub device_app_dir: Option<String>,
    /// The local app bundle, mapped onto the device one.
    pub local_app_dir: Option<PathBuf>,
    /// `unix-connect:///…/gdb.sock` or `connect://127.0.0.1:<port>`.
    pub broker_url: String,
}

impl AttachPlan {
    /// A device plan: `remote-ios`, `arm64-apple-ios`.
    pub fn ios(local_executable: PathBuf, broker_url: String) -> Self {
        Self {
            platform: LldbPlatform::RemoteIos { sysroot: None },
            arch: Some("arm64-apple-ios".to_owned()),
            local_executable,
            device_app_dir: None,
            local_app_dir: None,
            broker_url,
        }
    }
}

/// The settings of RD §8.4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitSettings {
    /// `partial` over USB, `minimal` over Wi-Fi (RD-19 decides the final default).
    pub memory_module_load_level: String,
    /// `plugin.process.gdb-remote.packet-timeout`, seconds: 30 over USB, 60 over Wi-Fi.
    pub packet_timeout_secs: u32,
    pub module_cache: Option<PathBuf>,
    /// `target.sdk-path` (Swift debug-info SDK lookup off macOS, RD V6).
    pub sdk_path: Option<PathBuf>,
}

impl Default for InitSettings {
    fn default() -> Self {
        Self {
            memory_module_load_level: "partial".to_owned(),
            packet_timeout_secs: 30,
            module_cache: None,
            sdk_path: None,
        }
    }
}

/// Quotes an argument for LLDB's command line: double quotes, with `\` and `"` escaped.
pub fn quote(arg: &str) -> String {
    let mut s = String::with_capacity(arg.len() + 2);
    s.push('"');
    for c in arg.chars() {
        if c == '"' || c == '\\' {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('"');
    s
}

fn quote_path(p: &Path) -> String {
    quote(&p.to_string_lossy())
}

/// RD §8.4.
pub fn init_commands(s: &InitSettings) -> Vec<String> {
    let mut out = vec![
        "settings set target.load-cwd-lldbinit false".to_owned(),
        format!("settings set target.memory-module-load-level {}", s.memory_module_load_level),
        format!("settings set plugin.process.gdb-remote.packet-timeout {}", s.packet_timeout_secs),
    ];
    if let Some(cache) = &s.module_cache {
        out.push(format!("settings set symbols.clang-modules-cache-path {}", quote_path(cache)));
    }
    if let Some(sdk) = &s.sdk_path {
        out.push(format!("settings set target.sdk-path {}", quote_path(sdk)));
    }
    out
}

/// The Python-free attach sequence (RD §9.1).
pub fn attach_commands(plan: &AttachPlan) -> Vec<String> {
    let mut out = Vec::new();
    match &plan.platform {
        LldbPlatform::RemoteIos { sysroot: Some(root) } => {
            out.push(format!("platform select remote-ios --sysroot {}", quote_path(root)));
        }
        LldbPlatform::RemoteIos { sysroot: None } => out.push("platform select remote-ios".to_owned()),
        LldbPlatform::Host => out.push("platform select host".to_owned()),
    }
    match &plan.arch {
        Some(arch) => out.push(format!("target create --arch {arch} {}", quote_path(&plan.local_executable))),
        None => out.push(format!("target create {}", quote_path(&plan.local_executable))),
    }
    if let (Some(device), Some(local)) = (&plan.device_app_dir, &plan.local_app_dir) {
        out.push(format!("target modules search-paths add {} {}", quote(device), quote_path(local)));
    }
    out.push(format!("process connect -p gdb-remote {}", quote(&plan.broker_url)));
    out
}

/// `attachCommands` for `lldb-dap` (RD §9.2): every line that must not fail carries the `!` prefix, which stops
/// the chain on failure.
pub fn dap_attach_commands(plan: &AttachPlan) -> Vec<String> {
    attach_commands(plan)
        .into_iter()
        .map(|c| if c.starts_with("target modules search-paths") { c } else { format!("!{c}") })
        .collect()
}

/// The M1 sequence over a plain forward (`attachMode: legacy-async`, ARCHITECTURE §10.3, §17.1). LLDB itself
/// sends `vAttach`, after an asynchronous connect to the process-less debugserver. Needs LLDB's Python.
pub fn legacy_async_commands(plan: &AttachPlan, pid: u32, device_executable: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    match &plan.platform {
        LldbPlatform::RemoteIos { .. } => out.push("platform select remote-ios".to_owned()),
        LldbPlatform::Host => out.push("platform select host".to_owned()),
    }
    match &plan.arch {
        Some(arch) => out.push(format!("target create --arch {arch} {}", quote_path(&plan.local_executable))),
        None => out.push(format!("target create {}", quote_path(&plan.local_executable))),
    }
    if let Some(remote) = device_executable {
        out.push(format!(
            "script lldb.target.module[0].SetPlatformFileSpec(lldb.SBFileSpec({}))",
            python_string(remote)
        ));
    }
    out.push("script lldb.debugger.SetAsync(True)".to_owned());
    out.push(format!("process connect -p gdb-remote {}", quote(&plan.broker_url)));
    out.push("script import time; time.sleep(2)".to_owned());
    out.push("script lldb.debugger.SetAsync(False)".to_owned());
    out.push(format!("process attach --pid {pid}"));
    out
}

fn python_string(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Whether any line uses LLDB's script interpreter: `script`, `command script …`, a Python breakpoint
/// command, or `-o script`. The golden test runs every non-legacy plan through it (PLAN §4.7).
pub fn uses_python(lines: &[String]) -> bool {
    lines.iter().any(|l| {
        let t = l.trim_start_matches('!').trim();
        let lower = t.to_ascii_lowercase();
        lower == "script"
            || lower.starts_with("script ")
            || lower.starts_with("command script")
            || (lower.starts_with("breakpoint command add")
                && (lower.contains("-s python") || lower.contains("--script-type python")))
            || lower.contains("-o script")
            || lower.starts_with("type summary add") && lower.contains("--python")
            || lower.starts_with("type synthetic add") && lower.contains("--python")
    })
}

/// A `--break FILE:LINE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakpoint {
    pub file: String,
    pub line: u32,
}

impl Breakpoint {
    /// Parses `FILE:LINE` (the last colon separates them, so Windows paths work).
    pub fn parse(spec: &str) -> Option<Self> {
        let (file, line) = spec.rsplit_once(':')?;
        let line: u32 = line.parse().ok()?;
        (!file.is_empty() && line > 0).then(|| Self { file: file.to_owned(), line })
    }

    pub fn command(&self) -> String {
        format!("breakpoint set --file {} --line {}", quote(&self.file), self.line)
    }
}

/// How a batch ends [D 84].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchEnd {
    /// `process detach`: the app keeps running (`--detach`).
    Detach,
    /// `process kill` (the default).
    Kill,
}

/// The `--batch` part of a session script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchSpec {
    pub commands: Vec<String>,
    pub end: BatchEnd,
}

/// The marker lines of the stop probe.
pub const STOP_TID: &str = "RUSSET-STOP-TID ";
pub const STOP_REASON: &str = "RUSSET-STOP-REASON ";
pub const FRAME_FUNCTION: &str = "RUSSET-FRAME-FUNCTION ";
pub const FRAME_FILE: &str = "RUSSET-FRAME-FILE ";
pub const FRAME_LINE: &str = "RUSSET-FRAME-LINE ";

/// Commands that print the current stop in a fixed format, then restore the formats.
pub fn stop_probe_commands() -> Vec<String> {
    vec![
        format!("settings set thread-format \"{STOP_TID}${{thread.id}}\\n{STOP_REASON}${{thread.stop-reason}}\\n\""),
        format!(
            "settings set frame-format \"{FRAME_FUNCTION}${{function.name-without-args}}\\n{FRAME_FILE}${{line.file.fullpath}}\\n{FRAME_LINE}${{line.number}}\\n\""
        ),
        "thread info".to_owned(),
        "frame info".to_owned(),
        "settings clear thread-format".to_owned(),
        "settings clear frame-format".to_owned(),
    ]
}

/// The full script `russet debug` runs: init, attach, breakpoints, `process continue` (unless
/// `stop_on_entry`), then for a batch the stop probe, the commands and the final detach or kill.
pub fn session_script(
    init: &[String],
    attach: &[String],
    breakpoints: &[Breakpoint],
    stop_on_entry: bool,
    batch: Option<&BatchSpec>,
) -> Vec<String> {
    let mut out: Vec<String> = init.to_vec();
    out.extend(attach.iter().cloned());
    out.extend(breakpoints.iter().map(Breakpoint::command));
    if !stop_on_entry {
        out.push("process continue".to_owned());
    }
    if let Some(b) = batch {
        out.extend(stop_probe_commands());
        out.extend(b.commands.iter().cloned());
        out.push(match b.end {
            BatchEnd::Detach => "process detach".to_owned(),
            BatchEnd::Kill => "process kill".to_owned(),
        });
    }
    out
}

/// The text of a script file: one command per line, with a provenance comment.
pub fn script_text(lines: &[String]) -> String {
    let mut s = String::from("# Generated by Russet. Python-free unless attach-mode is legacy-async.\n");
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s
}

/// The `lldb` arguments that run `script` in batch mode: `command source -e false` keeps going after a failing
/// command (LLDB's batch mode otherwise stops the script at the first error).
pub fn batch_args(script: &Path) -> Vec<String> {
    vec!["-x".to_owned(), "-b".to_owned(), "-o".to_owned(), format!("command source -e false {}", quote_path(script))]
}

/// The `lldb` arguments of an interactive session.
pub fn interactive_args(script: &Path) -> Vec<String> {
    vec!["-s".to_owned(), script.to_string_lossy().into_owned()]
}

/// One `--command` and what it printed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    pub command: String,
    pub output: String,
}

/// The frame of a stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// The first stop of a batch run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchStop {
    /// The first word of LLDB's stop reason: `breakpoint`, `signal`, `exception`, `trace`, …
    pub reason: String,
    /// LLDB's full stop reason, for example `breakpoint 1.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<u64>,
    pub frame: StopFrame,
}

/// What a batch transcript says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchTranscript {
    pub stop: Option<BatchStop>,
    pub commands: Vec<CommandOutput>,
    /// The final `process detach` succeeded.
    pub detached: bool,
    /// LLDB reported the process exited (by itself, or killed by the final `process kill`).
    pub exited: bool,
    pub exit_status: Option<i32>,
}

/// Splits LLDB's echoed batch output into commands and reads the stop probe. `user_commands` are the
/// `--command`s, in order; their outputs are returned in that order.
pub fn parse_batch_output(output: &str, user_commands: &[String]) -> BatchTranscript {
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(cmd) = line.strip_prefix("(lldb) ") {
            sections.push((cmd.trim().to_owned(), Vec::new()));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push(line);
        }
    }
    let mut t = BatchTranscript::default();
    let mut values: BTreeMap<&str, String> = BTreeMap::new();
    for (cmd, body) in &sections {
        for l in body {
            for key in [STOP_TID, STOP_REASON, FRAME_FUNCTION, FRAME_FILE, FRAME_LINE] {
                if let Some(v) = l.strip_prefix(key) {
                    values.entry(key).or_insert_with(|| v.trim().to_owned());
                }
            }
            if let Some(status) = exited_status(l) {
                t.exited = true;
                t.exit_status = Some(status);
            }
        }
        if cmd == "process detach" && body.iter().any(|l| l.contains(" detached")) {
            t.detached = true;
        }
    }
    if let Some(reason) = values.get(STOP_REASON) {
        let parse_num = |s: &str| match s.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16).ok(),
            None => s.parse().ok(),
        };
        t.stop = Some(BatchStop {
            reason: reason.split_whitespace().next().unwrap_or_default().to_owned(),
            description: Some(reason.clone()),
            thread_id: values.get(STOP_TID).and_then(|v| parse_num(v)),
            frame: StopFrame {
                function: values.get(FRAME_FUNCTION).filter(|v| !v.is_empty()).cloned(),
                file: values.get(FRAME_FILE).filter(|v| !v.is_empty()).cloned(),
                line: values.get(FRAME_LINE).and_then(|v| v.parse().ok()),
            },
        });
    }
    // Each user command's section, in order, after the stop probe (whose `thread info` and `frame info` would
    // otherwise answer a user command with the same text); a command may appear twice, so take them in sequence.
    let mut cursor = sections.iter().rposition(|(c, _)| c == "settings clear frame-format").map(|i| i + 1).unwrap_or(0);
    for wanted in user_commands {
        let found = sections[cursor..].iter().position(|(c, _)| c == wanted.trim());
        match found {
            Some(i) => {
                let (cmd, body) = &sections[cursor + i];
                t.commands.push(CommandOutput { command: cmd.clone(), output: body.join("\n").trim_end().to_owned() });
                cursor += i + 1;
            }
            None => t.commands.push(CommandOutput { command: wanted.clone(), output: String::new() }),
        }
    }
    t
}

/// `Process 1234 exited with status = 9 (0x00000009) killed` → 9.
fn exited_status(line: &str) -> Option<i32> {
    let rest = line.trim().strip_prefix("Process ")?;
    let (_, after) = rest.split_once(" exited with status = ")?;
    after.split_whitespace().next()?.parse().ok()
}

/// What ended a batch run before LLDB finished by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchCut {
    /// The time limit (`russet debug --timeout`) passed.
    Timeout(Duration),
    /// Ctrl-C.
    Interrupted,
}

/// A finished batch run of LLDB.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchRun {
    /// LLDB's exit code; `None` when a signal ended it. Batch mode exits 0 even when commands fail, so the
    /// transcript says more ([`batch_failure`]).
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// What ended the run early, if anything.
    pub cut: Option<BatchCut>,
    /// LLDB was still running a grace period after the session ended under it, and was killed.
    pub killed: bool,
}

/// How long LLDB gets to finish once the session ended under it, by default.
pub const BATCH_GRACE: Duration = Duration::from_secs(10);
/// `russet debug --batch`'s default time limit (`--timeout`).
pub const BATCH_TIMEOUT: Duration = Duration::from_secs(120);

/// The time limits of [`run_batch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchLimits {
    /// The whole run: connect, the first stop and the commands. `None`: no limit.
    pub timeout: Option<Duration>,
    /// How long LLDB gets to exit after the session ended under it.
    pub grace: Duration,
}

impl Default for BatchLimits {
    fn default() -> Self {
        Self { timeout: Some(BATCH_TIMEOUT), grace: BATCH_GRACE }
    }
}

/// Runs LLDB in batch mode: stdin closed, output captured, and on Unix in its own process group, so a terminal
/// Ctrl-C reaches only Russet, which decides what happens. When the time limit passes or `interrupt` resolves
/// first, `end_session` ends the session under LLDB (`run.stop`): the broker kills or detaches the app and drops
/// LLDB's connection, so a `process continue` that waits for a stop returns, and the script's remaining commands
/// fail fast. LLDB then gets `limits.grace` to exit before it is killed.
pub async fn run_batch<I, E, F>(
    mut cmd: tokio::process::Command,
    limits: BatchLimits,
    interrupt: I,
    end_session: E,
) -> std::io::Result<BatchRun>
where
    I: Future<Output = ()>,
    E: FnOnce(BatchCut) -> F,
    F: Future<Output = ()>,
{
    use tokio::io::AsyncReadExt as _;
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn()?;
    let drain = |pipe: Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf).await;
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>));
    let timeout = limits.timeout;
    let deadline = async {
        match timeout {
            Some(t) => tokio::time::sleep(t).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(interrupt, deadline);
    let (status, cut) = tokio::select! {
        status = child.wait() => (Some(status?), None),
        () = &mut deadline => (None, Some(BatchCut::Timeout(timeout.unwrap_or_default()))),
        () = &mut interrupt => (None, Some(BatchCut::Interrupted)),
    };
    let mut killed = false;
    let status = match (status, cut) {
        (Some(status), _) => status,
        (None, cut) => {
            if let Some(cut) = cut {
                end_session(cut).await;
            }
            match tokio::time::timeout(limits.grace, child.wait()).await {
                Ok(status) => status?,
                Err(_) => {
                    killed = true;
                    child.kill().await?;
                    child.wait().await?
                }
            }
        }
    };
    let collect = |h: tokio::task::JoinHandle<Vec<u8>>| async move {
        // A grandchild could hold a pipe open; never wait for it long.
        let abort = h.abort_handle();
        match tokio::time::timeout(Duration::from_secs(2), h).await {
            Ok(Ok(b)) => String::from_utf8_lossy(&b).into_owned(),
            _ => {
                abort.abort();
                String::new()
            }
        }
    };
    let (stdout, stderr) = (collect(out).await, collect(err).await);
    Ok(BatchRun { code: status.code(), stdout, stderr, cut, killed })
}

/// Why a batch run did not do its job, if it did not: stop at the first stop and run the commands there
/// (ARCHITECTURE §8.7). In order:
///
/// - Ctrl-C: `core.cancelled` (exit 130);
/// - the time limit: `debug.batchTimeout`;
/// - no stop: `debug.processExited` when LLDB saw the app exit first, `debug.lldbUnsupported` when LLDB lacks
///   the platform, else `debug.brokerLost` (LLDB could not reach the session's broker, or lost it).
///
/// `details` carries LLDB's first error lines and its exit status.
pub fn batch_failure(t: &BatchTranscript, run: &BatchRun) -> Option<russet_core::Error> {
    use russet_core::error::codes;
    let lldb_errors: Vec<&str> = run.stderr.lines().filter(|l| l.starts_with("error:")).take(8).collect();
    let mut details = serde_json::json!({
        "stopped": t.stop.is_some(),
        "lldbErrors": lldb_errors,
        "lldbExit": run.code,
        "lldbKilled": run.killed,
    });
    match run.cut {
        Some(BatchCut::Interrupted) => {
            return Some(russet_core::Error::new(codes::core::CANCELLED, "interrupted: the batch was stopped"));
        }
        Some(BatchCut::Timeout(limit)) => {
            details["timeoutSecs"] = limit.as_secs().into();
            let what = if t.stop.is_some() { "finish its commands" } else { "stop" };
            return Some(
                crate::errors::error(
                    codes::debug::BATCH_TIMEOUT,
                    format!("the batch did not {what} within {} s", limit.as_secs()),
                )
                .with_details(details),
            );
        }
        None => {}
    }
    if t.stop.is_some() {
        return None;
    }
    if t.exited {
        let status = t.exit_status.map(|s| format!(" with status {s}")).unwrap_or_default();
        return Some(
            crate::errors::error(codes::debug::PROCESS_EXITED, format!("the app exited{status} before it stopped"))
                .with_hint("Check the --break FILE:LINE, and the app's output with `russet logs --session`.")
                .with_details(details),
        );
    }
    if run.stderr.contains("unable to find a plug-in for the platform") {
        return Some(
            crate::errors::error(codes::debug::LLDB_UNSUPPORTED, "this LLDB has no remote-ios platform")
                .with_details(details),
        );
    }
    Some(
        crate::errors::error(codes::debug::BROKER_LOST, "LLDB never stopped the app: it could not reach the broker")
            .with_hint("Check the session with `russet sessions` and run again; LLDB's errors are in the details.")
            .with_details(details),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> AttachPlan {
        AttachPlan {
            device_app_dir: Some("/private/var/containers/Bundle/Application/1C2/RussetProbe.app".into()),
            local_app_dir: Some(PathBuf::from(
                "/home/u/RussetProbe/.russet/build/debug-iphoneos/signed/RussetProbe.app",
            )),
            ..AttachPlan::ios(
                PathBuf::from("/home/u/RussetProbe/.russet/build/debug-iphoneos/RussetProbe.app/RussetProbe"),
                "unix-connect:///run/user/1000/russet/sessions/ses_7k2/gdb.sock".into(),
            )
        }
    }

    #[test]
    fn attach_commands_match_rd_9_1() {
        assert_eq!(
            attach_commands(&plan()),
            vec![
                "platform select remote-ios",
                "target create --arch arm64-apple-ios \"/home/u/RussetProbe/.russet/build/debug-iphoneos/RussetProbe.app/RussetProbe\"",
                "target modules search-paths add \"/private/var/containers/Bundle/Application/1C2/RussetProbe.app\" \"/home/u/RussetProbe/.russet/build/debug-iphoneos/signed/RussetProbe.app\"",
                "process connect -p gdb-remote \"unix-connect:///run/user/1000/russet/sessions/ses_7k2/gdb.sock\"",
            ]
        );
        let sysroot = AttachPlan {
            platform: LldbPlatform::RemoteIos { sysroot: Some(PathBuf::from("/c/device-support/24A437/0D1C")) },
            ..plan()
        };
        assert_eq!(
            attach_commands(&sysroot)[0],
            "platform select remote-ios --sysroot \"/c/device-support/24A437/0D1C\""
        );
        let dap = dap_attach_commands(&plan());
        assert!(
            dap[0].starts_with("!platform")
                && dap[1].starts_with("!target create")
                && dap[3].starts_with("!process connect")
        );
        assert!(dap[2].starts_with("target modules search-paths"));
    }

    /// The golden test of PLAN §4.7: generated scripts contain no `script`, `command script` or Python breakpoint
    /// commands (the legacy-async fallback is the documented exception).
    #[test]
    fn generated_scripts_are_python_free() {
        let init = init_commands(&InitSettings {
            module_cache: Some(PathBuf::from("/c/lldb-module-cache")),
            sdk_path: Some(PathBuf::from("/sdk/iPhoneOS27.0.sdk")),
            ..InitSettings::default()
        });
        let bps = vec![Breakpoint::parse("ContentView.swift:12").unwrap_or_else(|| unreachable!())];
        for batch in
            [None, Some(BatchSpec { commands: vec!["frame variable probeValue".into()], end: BatchEnd::Detach })]
        {
            let script = session_script(&init, &attach_commands(&plan()), &bps, false, batch.as_ref());
            assert!(!uses_python(&script), "{script:#?}");
            assert!(!script_text(&script).lines().any(|l| l.trim_start().starts_with("script")));
        }
        assert!(!uses_python(&dap_attach_commands(&plan())));
        assert!(uses_python(&legacy_async_commands(&plan(), 932, Some("/private/var/x/RussetProbe"))));
        // The detector itself.
        for bad in ["script print(1)", "command script import x.py", "breakpoint command add -s python 1", "!script x"]
        {
            assert!(uses_python(&[bad.to_owned()]), "{bad}");
        }
        assert!(!uses_python(&["settings set target.memory-module-load-level partial".to_owned()]));
    }

    #[test]
    fn batch_scripts_end_with_detach_or_kill_after_the_commands() {
        let attach = attach_commands(&plan());
        let bps = vec![Breakpoint { file: "ContentView.swift".into(), line: 12 }];
        let batch =
            BatchSpec { commands: vec!["frame variable probeValue".into(), "bt".into()], end: BatchEnd::Detach };
        let s = session_script(&[], &attach, &bps, false, Some(&batch));
        let pos = |c: &str| s.iter().position(|l| l == c).unwrap_or(usize::MAX);
        assert!(pos("breakpoint set --file \"ContentView.swift\" --line 12") < pos("process continue"));
        assert!(pos("process continue") < pos("thread info"));
        assert!(pos("thread info") < pos("frame variable probeValue"));
        assert_eq!(s.last().map(String::as_str), Some("process detach"));
        let kill = BatchSpec { end: BatchEnd::Kill, ..batch };
        assert_eq!(
            session_script(&[], &attach, &bps, false, Some(&kill)).last().map(String::as_str),
            Some("process kill")
        );
        // --stop-on-entry: no continue.
        assert!(!session_script(&[], &attach, &bps, true, None).contains(&"process continue".to_owned()));
        assert_eq!(batch_args(Path::new("/tmp/a b/s.lldb"))[3], "command source -e false \"/tmp/a b/s.lldb\"");
    }

    #[test]
    fn quoting_and_breakpoints() {
        assert_eq!(quote(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        assert_eq!(
            Breakpoint::parse("C:\\src\\a.swift:7"),
            Some(Breakpoint { file: "C:\\src\\a.swift".into(), line: 7 })
        );
        assert_eq!(Breakpoint::parse("a.swift"), None);
        assert_eq!(Breakpoint::parse("a.swift:0"), None);
        assert_eq!(python_string("it's"), r"'it\'s'");
    }

    /// A transcript captured from Swift 6.4's LLDB 21.0.0 on the host (probe.c through lldb-server, 2026-10-11).
    const TRANSCRIPT: &str = "(lldb) command source -e false \"/tmp/s.lldb\"
Executing commands in '/tmp/s.lldb'.
(lldb) process connect -p gdb-remote connect://127.0.0.1:35655
Process 1134583 stopped
* thread #1, name = 'probe', stop reason = signal SIGSTOP
Target 0: (probe) stopped.
(lldb) breakpoint set --file \"probe.c\" --line 25
Breakpoint 1: where = probe`probe_step + 16 at probe.c:25:21, address = 0x0000555555555199
(lldb) process continue
Process 1134583 resuming
Process 1134583 stopped
* thread #1, name = 'probe', stop reason = breakpoint 1.1
-> 25  \t  return probeValue + seq;
Target 0: (probe) stopped.
(lldb) settings set thread-format \"RUSSET-STOP-TID ${thread.id}\\nRUSSET-STOP-REASON ${thread.stop-reason}\\n\"
(lldb) settings set frame-format \"x\"
(lldb) thread info
RUSSET-STOP-TID 0x114ff7
RUSSET-STOP-REASON breakpoint 1.1

(lldb) frame info
RUSSET-FRAME-FUNCTION probe_step
RUSSET-FRAME-FILE /tmp/exp1/probe.c
RUSSET-FRAME-LINE 25
(lldb) settings clear thread-format
(lldb) settings clear frame-format
(lldb) frame variable nothing
error: no variable named 'nothing' found in this frame
(lldb) frame variable probeValue
(int) probeValue = 42
(lldb) process detach
Process 1134583 detached
";

    #[test]
    fn batch_transcripts_parse() {
        let cmds = vec!["frame variable probeValue".to_owned(), "frame variable nothing".to_owned()];
        let t = parse_batch_output(TRANSCRIPT, &cmds);
        let stop = t.stop.clone().unwrap_or_else(|| unreachable!());
        assert_eq!(stop.reason, "breakpoint");
        assert_eq!(stop.thread_id, Some(0x114ff7));
        assert_eq!(
            stop.frame,
            StopFrame { function: Some("probe_step".into()), file: Some("/tmp/exp1/probe.c".into()), line: Some(25) }
        );
        assert!(t.detached && !t.exited);
        // In the order asked, even though they ran in another order.
        assert_eq!(t.commands[0].output, "(int) probeValue = 42");
        assert_eq!(t.commands[1].command, "frame variable nothing");
        assert_eq!(t.commands[1].output, "", "the second command is searched after the first one");
        let killed = TRANSCRIPT.replace(
            "(lldb) process detach\nProcess 1134583 detached\n",
            "(lldb) process kill\nProcess 1134583 exited with status = 9 (0x00000009) killed\n",
        );
        let t = parse_batch_output(&killed, &cmds[..1]);
        assert!(t.exited && !t.detached);
        assert_eq!(t.exit_status, Some(9));
    }

    #[test]
    fn user_commands_are_not_confused_with_the_probe() {
        let out = "(lldb) process continue\nProcess 7 stopped\n(lldb) thread info\nRUSSET-STOP-TID 0x1\nRUSSET-STOP-REASON breakpoint 1.1\n\
                   (lldb) frame info\nRUSSET-FRAME-LINE 12\n(lldb) settings clear thread-format\n(lldb) settings clear frame-format\n\
                   (lldb) frame info\nframe #0: 0x0000000100004000 RussetProbe`ContentView.body.getter at ContentView.swift:12:9\n\
                   (lldb) process detach\nProcess 7 detached\n";
        let t = parse_batch_output(out, &["frame info".to_owned()]);
        assert!(t.commands[0].output.starts_with("frame #0:"), "{:?}", t.commands);
        assert_eq!(t.stop.unwrap_or_else(|| unreachable!()).frame.line, Some(12));
    }

    #[cfg(unix)]
    fn sh(script: &str) -> tokio::process::Command {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(script);
        c
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_batch_run_captures_lldbs_output_and_status() {
        let run = run_batch(
            sh("echo out; echo 'error: x' >&2; exit 3"),
            BatchLimits::default(),
            std::future::pending(),
            |_| async {},
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((run.code, run.stdout.as_str(), run.stderr.as_str()), (Some(3), "out\n", "error: x\n"));
        assert_eq!((run.cut, run.killed), (None, false));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_time_limit_ends_the_session_and_lldb_follows() {
        let flag = std::env::temp_dir().join(format!("russet-batch-{}", std::process::id()));
        let _ = std::fs::remove_file(&flag);
        let mut cmd = sh("while [ ! -e \"$F\" ]; do sleep 0.02; done; echo 'Process 7 exited with status = -1'");
        cmd.env("F", &flag);
        let limits = BatchLimits { timeout: Some(Duration::from_millis(200)), grace: Duration::from_secs(10) };
        let ended = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen = std::sync::Arc::clone(&ended);
        let f = flag.clone();
        let run = run_batch(cmd, limits, std::future::pending(), move |cut| async move {
            // What run.stop does to LLDB: its connection drops and `process continue` returns.
            *seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cut);
            let _ = std::fs::write(&f, b"");
        })
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        let _ = std::fs::remove_file(&flag);
        let cut = Some(BatchCut::Timeout(Duration::from_millis(200)));
        assert_eq!(*ended.lock().unwrap_or_else(std::sync::PoisonError::into_inner), cut);
        assert_eq!((run.cut, run.killed, run.code), (cut, false, Some(0)));
        assert!(run.stdout.contains("exited"));
        let e = batch_failure(&parse_batch_output(&run.stdout, &[]), &run).unwrap_or_else(|| unreachable!());
        assert_eq!(e.code(), russet_core::error::codes::debug::BATCH_TIMEOUT);
        assert_eq!(e.to_data().details.unwrap_or_default()["timeoutSecs"], 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_interrupt_cuts_the_run_and_a_stuck_lldb_is_killed() {
        let started = std::time::Instant::now();
        let limits = BatchLimits { timeout: None, grace: Duration::from_millis(300) };
        let interrupt = tokio::time::sleep(Duration::from_millis(100));
        let run = run_batch(sh("trap '' INT TERM; sleep 30"), limits, interrupt, |_| async {})
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((run.cut, run.killed, run.code), (Some(BatchCut::Interrupted), true, None));
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        let e = batch_failure(&BatchTranscript::default(), &run).unwrap_or_else(|| unreachable!());
        assert_eq!(e.code(), russet_core::error::codes::core::CANCELLED);
    }

    #[test]
    fn batch_failures_name_what_went_wrong() {
        use russet_core::error::codes;
        let run = |stderr: &str| BatchRun { code: Some(0), stderr: stderr.into(), ..BatchRun::default() };
        let code = |t: &BatchTranscript, r: &BatchRun| batch_failure(t, r).map(|e| e.code());
        // The batch stopped: success, whatever LLDB's errors (a failing --command is part of the output).
        let stopped = parse_batch_output(TRANSCRIPT, &[]);
        assert_eq!(code(&stopped, &run("error: no variable named 'nothing'")), None);
        // The app exited before any stop.
        let exited = BatchTranscript { exited: true, exit_status: Some(0), ..BatchTranscript::default() };
        assert_eq!(code(&exited, &run("")), Some(codes::debug::PROCESS_EXITED));
        // LLDB without the platform; LLDB that could not reach the broker (captured from LLDB 21.0.0).
        let none = BatchTranscript::default();
        let platform = run("error: unable to find a plug-in for the platform named \"remote-ios\"\n");
        assert_eq!(code(&none, &platform), Some(codes::debug::LLDB_UNSUPPORTED));
        let unreachable = run("error: No such file or directory\nerror: Process must be launched.\n");
        let e = batch_failure(&none, &unreachable).unwrap_or_else(|| unreachable!());
        assert_eq!(e.code(), codes::debug::BROKER_LOST);
        let details = e.to_data().details.unwrap_or_default();
        assert_eq!(details["lldbErrors"][0], "error: No such file or directory");
        assert_eq!(details["stopped"], false);
        // A timeout after the stop says so.
        let late = BatchRun { cut: Some(BatchCut::Timeout(Duration::from_secs(120))), ..run("") };
        let e = batch_failure(&stopped, &late).unwrap_or_else(|| unreachable!());
        assert_eq!(e.code(), codes::debug::BATCH_TIMEOUT);
        assert!(e.message().contains("finish its commands within 120 s"), "{}", e.message());
        assert!(e.hint().is_some());
    }

    #[test]
    fn a_process_that_exits_before_any_stop_has_no_stop() {
        let out = "(lldb) process continue\nProcess 7 resuming\nProcess 7 exited with status = 0 (0x00000000)\n(lldb) thread info\nerror: invalid process\n";
        let t = parse_batch_output(out, &[]);
        assert!(t.stop.is_none());
        assert!(t.exited);
        assert_eq!(t.exit_status, Some(0));
    }
}
