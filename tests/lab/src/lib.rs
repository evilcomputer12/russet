// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet-lab`: the device-in-the-loop harness and suites (QA §3.9, L7). Lane A only: suites run through
//! `cargo xtask lab` against the live daemon of lane A's `RUSSET_HOME`, under the live-device lock (ARCHITECTURE
//! §3.3 item 7). The lab's own exit convention is 3 = skipped environment (ARCHITECTURE §13.4).
//!
//! Wave 1 (W9): [`LabHarness`] drives the `russet` CLI (`--json`) of `RUSSET_BIN`; the `smoke` suite
//! (`tests/smoke.rs`) holds QA's E2E-01, 02, 03, 04, 06 and 14, adapted to the Wave-1 bundle-ID mapping: RussetProbe
//! runs under the owner-local `russet.local.toml` (PLAN §4.10), so its effective bundle ID is the `XTL-…` ID of the
//! existing xtool profile, which is the designated test slot and stays installed; E2E-14 removes
//! `dev.russet.test.*` apps and every session.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Why a lab step could not pass.
#[derive(Debug)]
pub enum LabError {
    /// The environment is not ready (not lane A, phone locked, …): exit 3, never a failure.
    SkippedEnv(String),
    /// A gate criterion failed.
    Failed(String),
}

impl std::fmt::Display for LabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SkippedEnv(m) => write!(f, "skipped-env: {m}"),
            Self::Failed(m) => write!(f, "failed: {m}"),
        }
    }
}

impl std::error::Error for LabError {}

/// A lab result.
pub type LabResult<T> = Result<T, LabError>;

fn failed(m: impl Into<String>) -> LabError {
    LabError::Failed(m.into())
}

/// One `russet … --json` call.
#[derive(Debug, Clone)]
pub struct CliOutcome {
    pub status: i32,
    /// The JSON document (the last line for NDJSON); `Null` when stdout was not JSON.
    pub json: Value,
    pub stdout: String,
    pub stderr: String,
    pub elapsed: Duration,
}

impl CliOutcome {
    /// The JSON result of a successful call.
    pub fn ok(self) -> LabResult<Value> {
        if self.status == 0 {
            Ok(self.json)
        } else {
            Err(failed(format!("exit {}: {} {}", self.status, self.stdout.trim(), self.stderr.trim())))
        }
    }

    /// Every line of NDJSON output that parses.
    pub fn ndjson(&self) -> Vec<Value> {
        self.stdout.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }
}

/// The lane-A harness: the `russet` binary, lane A's home and the project under test.
#[derive(Debug, Clone)]
pub struct LabHarness {
    pub bin: PathBuf,
    pub home: PathBuf,
    /// RussetProbe in this repository.
    pub probe: PathBuf,
    /// `PATH` without `$HOME/.local/bin` (xtool, pymobiledevice3, go-ios).
    path: String,
}

/// `PATH` without `hidden`.
pub fn path_without(path: &str, hidden: &Path) -> String {
    std::env::split_paths(path)
        .filter(|p| p != hidden)
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":")
}

impl LabHarness {
    /// The harness, or `SkippedEnv` outside lane A (`RUSSET_LANE=A`, `RUSSET_HOME`, `RUSSET_BIN`).
    pub fn acquire() -> LabResult<Self> {
        if std::env::var("RUSSET_LANE").as_deref() != Ok("A") {
            return Err(LabError::SkippedEnv("lane A only: RUSSET_LANE=A is not set".into()));
        }
        let home = std::env::var_os("RUSSET_HOME")
            .map(PathBuf::from)
            .ok_or_else(|| LabError::SkippedEnv("RUSSET_HOME is not set".into()))?;
        let bin = std::env::var_os("RUSSET_BIN")
            .map(PathBuf::from)
            .ok_or_else(|| LabError::SkippedEnv("RUSSET_BIN is not set (cargo xtask lab sets it)".into()))?;
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let probe = repo.join("tests/fixtures/apps/RussetProbe").canonicalize().map_err(|e| failed(e.to_string()))?;
        if !probe.join("russet.local.toml").is_file() {
            return Err(LabError::SkippedEnv(
                "tests/fixtures/apps/RussetProbe/russet.local.toml is missing (PLAN §4.10)".into(),
            ));
        }
        let hidden = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin")).unwrap_or_default();
        let path = path_without(&std::env::var("PATH").unwrap_or_default(), &hidden);
        Ok(Self { bin, home, probe, path })
    }

    /// Runs `russet <args> --json` in the RussetProbe project.
    pub fn cli(&self, args: &[&str]) -> LabResult<CliOutcome> {
        let started = Instant::now();
        let out = Command::new(&self.bin)
            .args(args)
            .arg("--json")
            .current_dir(&self.probe)
            .env("PATH", &self.path)
            .env("RUSSET_HOME", &self.home)
            .env("RUSSET_LANE", "A")
            .output()
            .map_err(|e| failed(format!("cannot run {}: {e}", self.bin.display())))?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let json = serde_json::from_str(stdout.trim())
            .or_else(|_| stdout.lines().last().map(serde_json::from_str).unwrap_or(Ok(Value::Null)))
            .unwrap_or(Value::Null);
        Ok(CliOutcome {
            status: out.status.code().unwrap_or(-1),
            json,
            stdout,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            elapsed: started.elapsed(),
        })
    }

    /// The one connected device's UDID (never printed).
    pub fn udid(&self) -> LabResult<String> {
        let list = self.cli(&["device", "list"])?.ok()?;
        list["devices"][0]["udid"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| LabError::SkippedEnv("no device is connected".into()))
    }

    /// RussetProbe's effective bundle ID (after the bundle-ID policy), from `russet build`.
    pub fn probe_bundle_id(&self) -> LabResult<String> {
        let b = self.cli(&["build"])?.ok()?;
        b["app"]["bundleId"].as_str().map(str::to_owned).ok_or_else(|| failed("russet build reported no bundleId"))
    }

    /// The line after the `RUSSET_PROBE_BREAK` code marker in ContentView.swift.
    pub fn probe_break_line(&self) -> LabResult<u32> {
        let src = std::fs::read_to_string(self.probe.join("Sources/RussetProbe/ContentView.swift"))
            .map_err(|e| failed(e.to_string()))?;
        probe_break_line(&src).ok_or_else(|| failed("no RUSSET_PROBE_BREAK marker"))
    }

    /// E2E-14: stops every session and uninstalls `dev.russet.test.*` apps.
    pub fn cleanup(&self) -> LabResult<Vec<String>> {
        let _ = self.cli(&["stop", "--all"])?;
        let apps = self.cli(&["app", "list"])?.ok()?;
        let mut removed = Vec::new();
        for id in installed_bundle_ids(&apps).into_iter().filter(|b| b.starts_with("dev.russet.test.")) {
            self.cli(&["app", "uninstall", &id])?.ok()?;
            removed.push(id);
        }
        Ok(removed)
    }
}

/// The 1-based line after the code marker (`// RUSSET_PROBE_BREAK` at the end of a line).
pub fn probe_break_line(src: &str) -> Option<u32> {
    src.lines().position(|l| l.trim_end().ends_with("// RUSSET_PROBE_BREAK")).map(|i| i as u32 + 2)
}

/// The bundle IDs of an `app list --json` result.
pub fn installed_bundle_ids(apps: &Value) -> Vec<String> {
    apps["apps"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["bundleId"].as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// Heartbeat facts of a list of log entries: the longest run of consecutive `seq` values and the p95 delivery
/// latency in ms (host receipt time minus the app's `t`, relative to the smallest such offset, which removes the
/// clock offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heartbeats {
    pub lines: usize,
    pub longest_run: usize,
    pub p95_latency_ms: Option<i64>,
}

/// Parses `PROBE seq=<n> t=<ms>` lines out of log entries (`message`, `ts` RFC 3339 with milliseconds).
pub fn heartbeats(entries: &[Value]) -> Heartbeats {
    let mut beats: Vec<(u64, i64)> = Vec::new();
    for e in entries {
        let Some(msg) = e["message"].as_str() else { continue };
        let Some(rest) = msg.strip_prefix("PROBE seq=") else { continue };
        let mut it = rest.split(" t=");
        let (Some(seq), Some(t)) = (it.next(), it.next()) else { continue };
        let (Ok(seq), Ok(t)) = (seq.trim().parse::<u64>(), t.trim().parse::<i64>()) else { continue };
        let host = e["ts"].as_str().and_then(ts_millis).unwrap_or(0);
        beats.push((seq, host - t));
    }
    beats.sort_by_key(|b| b.0);
    beats.dedup_by_key(|b| b.0);
    let (mut best, mut run) = (0, 0);
    for (i, b) in beats.iter().enumerate() {
        run = if i > 0 && b.0 == beats[i - 1].0 + 1 { run + 1 } else { 1 };
        best = best.max(run);
    }
    let p95 = beats.iter().map(|b| b.1).min().map(|min| {
        let mut l: Vec<i64> = beats.iter().map(|b| b.1 - min).collect();
        l.sort_unstable();
        l[((l.len() * 95).div_ceil(100)).saturating_sub(1)]
    });
    Heartbeats { lines: beats.len(), longest_run: best, p95_latency_ms: p95 }
}

/// Milliseconds since the epoch of an RFC 3339 UTC timestamp (`2026-10-11T10:33:40.424Z`).
pub fn ts_millis(ts: &str) -> Option<i64> {
    let (date, time) = ts.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':').map(|x| x.parse::<i64>());
    let (h, mi, s) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    let ms: i64 = format!("{frac:0<3}")[..3].parse().ok()?;
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + s * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_the_line_after_the_code_marker_not_the_doc_comment() {
        let src = "/// the `RUSSET_PROBE_BREAK` marker\nstruct V {\n  let probeValue = 42  // RUSSET_PROBE_BREAK\n  Text()\n}\n";
        assert_eq!(probe_break_line(src), Some(4));
    }

    #[test]
    fn heartbeat_runs_and_latency() {
        let e = |seq: u64, t: i64, ts: &str| json!({"message": format!("PROBE seq={seq} t={t}"), "ts": ts});
        let entries = vec![
            e(0, 0, "2026-10-11T10:00:00.000Z"),
            e(1, 100, "2026-10-11T10:00:00.150Z"),
            e(2, 200, "2026-10-11T10:00:00.200Z"),
            e(4, 400, "2026-10-11T10:00:00.400Z"),
            json!({"message": "other", "ts": "2026-10-11T10:00:00.400Z"}),
        ];
        let h = heartbeats(&entries);
        assert_eq!((h.lines, h.longest_run), (4, 3));
        assert_eq!(h.p95_latency_ms, Some(50));
    }

    #[test]
    fn rfc3339_millis() {
        assert_eq!(ts_millis("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(ts_millis("2026-10-11T10:33:40.424Z"), Some(1_791_714_820_424));
    }

    #[test]
    fn path_filter_hides_only_the_named_directory() {
        assert_eq!(path_without("/a:/h/.local/bin:/b", Path::new("/h/.local/bin")), "/a:/b");
    }
}
