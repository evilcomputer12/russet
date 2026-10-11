// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask lab`: runs the device-in-the-loop suites through the live daemon of its own `RUSSET_HOME`; refuses while a daemon of another `RUSSET_HOME` holds the live-device lock (ARCHITECTURE §3.3 item 7). Lane A only.
//!
//! - `cargo xtask lab smoke`: the `russet-lab` smoke suite (E2E-01, 02, 03, 04, 06, 14), serially.
//! - `cargo xtask lab exit-script [--runs N]`: `demo/m1-russet.sh` N times in a row (DC-05, default 3); every run
//!   after the first must skip the install (the ledger).
//! - `cargo xtask lab dc28`: `tests/lab/dc28-ddi.sh` (gate item 7).
//!
//! Exit codes: 0 passed, 1 failed, 3 skipped environment (not lane A, the lock held by another home, the phone not
//! ready). Builds `russet` first and passes it to the suites as `RUSSET_BIN`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Flags of `cargo xtask lab`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The suite: `smoke`, `exit-script` or `dc28`.
    #[arg(default_value = "smoke")]
    pub suite: String,
    /// `exit-script`: consecutive runs.
    #[arg(long, default_value_t = 3)]
    pub runs: u32,
    /// Where the scripts write their records (default: a new directory under the system temp dir).
    #[arg(long)]
    pub out: Option<PathBuf>,
}

/// The skipped-environment exit (QA §3.9.1 item 4).
const SKIPPED_ENV: i32 = 3;

fn skip(why: &str) -> ! {
    eprintln!("lab: skipped-env: {why}");
    std::process::exit(SKIPPED_ENV)
}

pub fn run(args: Args) -> Result<()> {
    if std::env::var("RUSSET_LANE").as_deref() != Ok("A") {
        skip("lane A only (RUSSET_LANE=A, AGENTS.md §4)");
    }
    let Some(home) = std::env::var_os("RUSSET_HOME").map(PathBuf::from) else {
        skip("RUSSET_HOME is not set (lane A: $HOME/russet-lab/home-A)");
    };
    if let Some(holder) = foreign_holder(&live_device_lock_path(), &home) {
        skip(&format!("the live-device lock is held by pid {} for another RUSSET_HOME ({})", holder.0, holder.1));
    }
    let root = crate::workstreams::repo_root();
    let bin = build_russet(&root)?;
    let out = match args.out {
        Some(o) => o,
        None => std::env::temp_dir().join(format!("russet-lab-{}-{}", args.suite, std::process::id())),
    };
    std::fs::create_dir_all(&out)?;
    println!("lab: {} (records in {})", args.suite, out.display());
    match args.suite.as_str() {
        "smoke" => {
            let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .args(["test", "-p", "russet-lab", "--test", "smoke", "--", "--ignored", "--test-threads=1"])
                .current_dir(&root)
                .env("RUSSET_BIN", &bin)
                .status()?;
            if !status.success() {
                bail!("the smoke suite failed");
            }
        }
        "exit-script" => exit_script(&root, &bin, &out, args.runs)?,
        "dc28" => {
            let code = script(&root.join("tests/lab/dc28-ddi.sh"), &bin, &[("RUSSET_DC28_OUT", &out)])?;
            match code {
                0 => {}
                SKIPPED_ENV => {
                    eprintln!("lab: DC-28 is blocked on the owner (exit 3)");
                    std::process::exit(SKIPPED_ENV);
                }
                c => bail!("DC-28 failed (exit {c})"),
            }
        }
        other => bail!("unknown suite `{other}` (smoke, exit-script, dc28)"),
    }
    println!("lab: {} passed", args.suite);
    Ok(())
}

/// `demo/m1-russet.sh` `runs` times; runs after the first must not reinstall.
fn exit_script(root: &Path, bin: &Path, out: &Path, runs: u32) -> Result<()> {
    for i in 1..=runs {
        let dir = out.join(format!("run-{i}"));
        let code = script(&root.join("demo/m1-russet.sh"), bin, &[("RUSSET_M1_OUT", &dir)])?;
        if code != 0 {
            bail!("exit script run {i} of {runs} failed (exit {code}; records in {})", dir.display());
        }
        let summary: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("summary.json"))?)?;
        if i > 1 && summary["installed"] != false {
            bail!("run {i} reinstalled the app: the install ledger did not skip it");
        }
        println!("lab: exit script run {i}/{runs} passed: {summary}");
    }
    Ok(())
}

fn script(path: &Path, bin: &Path, env: &[(&str, &Path)]) -> Result<i32> {
    let mut cmd = Command::new(path);
    cmd.env("RUSSET_BIN", bin);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = cmd.status().with_context(|| format!("running {}", path.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// Builds this worktree's `russet` and returns its path.
fn build_russet(root: &Path) -> Result<PathBuf> {
    let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "-p", "russet-cli"])
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("building russet failed");
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let bin = target.join("debug").join(if cfg!(windows) { "russet.exe" } else { "russet" });
    if !bin.is_file() {
        bail!("no russet binary at {}", bin.display());
    }
    Ok(bin)
}

/// russetd's lock path (`daemon/lock.rs`): `RUSSET_LIVE_DEVICE_LOCK`, else `$XDG_RUNTIME_DIR/russet-live-device.lock`,
/// else `/tmp/russet-<uid>/russet-live-device.lock`.
fn live_device_lock_path() -> PathBuf {
    if let Some(p) = std::env::var_os("RUSSET_LIVE_DEVICE_LOCK").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
        Some(dir) => dir.join("russet-live-device.lock"),
        None => std::env::temp_dir().join(format!("russet-{}", uid())).join("russet-live-device.lock"),
    }
}

#[cfg(unix)]
fn uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0)
}

#[cfg(not(unix))]
fn uid() -> u32 {
    0
}

/// The lock's holder `(pid, home)` when it is a live process of another `RUSSET_HOME`.
fn foreign_holder(lock: &Path, home: &Path) -> Option<(u32, String)> {
    let text = std::fs::read_to_string(lock).ok()?;
    let (pid, held_home) = parse_holder(&text)?;
    let alive = Path::new("/proc").join(pid.to_string()).exists() || !cfg!(target_os = "linux");
    let same = match (Path::new(&held_home).canonicalize(), home.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => Path::new(&held_home) == home,
    };
    (alive && !same).then_some((pid, held_home))
}

/// The `{pid, home}` record russetd writes into the lock file.
fn parse_holder(text: &str) -> Option<(u32, String)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    Some((u32::try_from(v["pid"].as_u64()?).ok()?, v["home"].as_str()?.to_owned()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn holder_records_parse() {
        assert_eq!(parse_holder(r#"{"pid": 7, "home": "/h/A"}"#), Some((7, "/h/A".to_owned())));
        assert_eq!(parse_holder(""), None);
    }

    #[test]
    fn only_a_live_holder_of_another_home_is_foreign() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("lock");
        let me = std::process::id();
        std::fs::write(&lock, format!(r#"{{"pid": {me}, "home": "/elsewhere"}}"#)).unwrap();
        assert_eq!(foreign_holder(&lock, Path::new("/mine")), Some((me, "/elsewhere".to_owned())));
        assert_eq!(foreign_holder(&lock, Path::new("/elsewhere")), None);
        std::fs::write(&lock, r#"{"pid": 4294967, "home": "/elsewhere"}"#).unwrap();
        if cfg!(target_os = "linux") {
            assert_eq!(foreign_holder(&lock, Path::new("/mine")), None, "a dead holder does not count");
        }
        assert_eq!(foreign_holder(&dir.path().join("missing"), Path::new("/mine")), None);
    }
}
