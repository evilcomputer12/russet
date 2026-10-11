// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask sweep`: keeps the cargo target slots under their cap (QA-24; PLAN §0 rule 7; AGENTS.md §6).
//!
//! For every slot (`--slot DIR`, default every `~/.cache/russet-dev/target-*`, plus `CARGO_TARGET_DIR`):
//! 1. take cargo's own build locks without waiting (`<profile>/.cargo-lock`, `.cargo-build-lock`,
//!    `.cargo-artifact-lock`); a slot whose lock is held by a running build is skipped, never swept under it;
//! 2. `cargo sweep --time N` when cargo-sweep is installed; otherwise the built-in equivalent removes the
//!    entries of `deps/`, `build/`, `.fingerprint/`, `examples/` and `incremental/` whose newest file is older
//!    than N days (the dev box mounts /home `noatime`, so modification times are the only usage signal);
//! 3. measure the slot against the cap (6 GiB); with `--clean-over-cap`, a slot still over the cap loses its
//!    profile directories (what `cargo clean` would remove), keeping the lock files.
//!
//! The cargo wrappers (`cargo xtask ci|test|build`) call [`sweep_if_due`] first, which runs step 2 at most once
//! per `--time` days per slot (stamp file `.russet-sweep`), so the weekly sweep happens without a scheduler.
//! Only Russet's own slots are touched (AGENTS.md §6: never touch files Russet did not create). Owner: W8.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};

use crate::disk_guard::GIB;

/// The stamp file of the last sweep, inside each slot.
const STAMP: &str = ".russet-sweep";
/// Cargo's lock files inside a profile directory (cargo 1.99).
const CARGO_LOCKS: &[&str] = &[".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"];
/// Per-unit artifact directories inside a profile directory.
const UNIT_DIRS: &[&str] = &["deps", "build", ".fingerprint", "examples", "incremental"];
/// Profile directories of a target dir (and host-target subdirectories cargo creates for cross builds).
const PROFILE_DIRS: &[&str] = &["debug", "release"];

/// Flags of `cargo xtask sweep`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Remove artifacts not rebuilt for this many days (`cargo sweep --time`)
    #[arg(long, default_value_t = 7)]
    pub time: u64,
    /// The cap per slot, in GiB
    #[arg(long, value_name = "GiB", default_value_t = 6.0)]
    pub max_gib: f64,
    /// Slot directories (default: every ~/.cache/russet-dev/target-* and CARGO_TARGET_DIR)
    #[arg(long = "slot", value_name = "DIR")]
    pub slots: Vec<PathBuf>,
    /// Clean a slot that is still over the cap after sweeping (as `cargo clean` would)
    #[arg(long)]
    pub clean_over_cap: bool,
    /// Report what would be removed without removing anything
    #[arg(long)]
    pub dry_run: bool,
}

/// What happened to one slot.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotReport {
    pub slot: PathBuf,
    pub before: u64,
    pub after: u64,
    pub removed_entries: usize,
    pub method: &'static str,
    pub busy: bool,
    pub cleaned: bool,
    pub over_cap: bool,
}

pub fn run(args: Args) -> Result<()> {
    let slots = if args.slots.is_empty() { default_slots() } else { args.slots.clone() };
    if slots.is_empty() {
        println!("sweep: no target slots found (~/.cache/russet-dev/target-*, CARGO_TARGET_DIR)");
        return Ok(());
    }
    let cap = (args.max_gib * GIB as f64) as u64;
    let mut over = Vec::new();
    for slot in &slots {
        let opts = Options { days: args.time, cap, clean_over_cap: args.clean_over_cap, dry_run: args.dry_run };
        let r = sweep_slot(slot, &opts).with_context(|| format!("sweeping {}", slot.display()))?;
        println!("{}", describe(&r, cap));
        if r.over_cap && !r.cleaned {
            over.push(r.slot.display().to_string());
        }
    }
    if !over.is_empty() {
        println!(
            "warning: still over the {} GiB cap: {} (rerun with --clean-over-cap when no build is running)",
            args.max_gib,
            over.join(", ")
        );
    }
    Ok(())
}

/// Options of one slot's sweep.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub days: u64,
    pub cap: u64,
    pub clean_over_cap: bool,
    pub dry_run: bool,
}

fn describe(r: &SlotReport, cap: u64) -> String {
    let gib = |b: u64| b as f64 / GIB as f64;
    if r.busy {
        return format!("sweep: {}: busy (a cargo build holds its lock); skipped", r.slot.display());
    }
    let mut s = format!(
        "sweep: {}: {:.2} GiB -> {:.2} GiB ({}, {} entries removed; cap {:.1} GiB)",
        r.slot.display(),
        gib(r.before),
        gib(r.after),
        r.method,
        r.removed_entries,
        gib(cap)
    );
    if r.cleaned {
        s.push_str("; over the cap: profile directories cleaned");
    } else if r.over_cap {
        s.push_str("; OVER THE CAP");
    }
    s
}

/// Every existing Russet target slot: `~/.cache/russet-dev/target-*` and `CARGO_TARGET_DIR`.
pub fn default_slots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let base = PathBuf::from(home).join(".cache/russet-dev");
        if let Ok(rd) = fs::read_dir(&base) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("target-") && e.path().is_dir() {
                    out.push(e.path());
                }
            }
        }
    }
    if let Some(t) = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from)
        && t.is_dir()
        && !out.iter().any(|o| same_dir(o, &t))
    {
        out.push(t);
    }
    out.sort();
    out
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Runs the time-based sweep on `slot` when its stamp is older than `days` (or missing). Used by the cargo
/// wrappers; never cleans, and never fails the caller: problems become a returned note.
pub fn sweep_if_due(slot: &Path, days: u64) -> Option<String> {
    if !slot.is_dir() {
        return None;
    }
    let due = match fs::metadata(slot.join(STAMP)).and_then(|m| m.modified()) {
        Ok(t) => SystemTime::now().duration_since(t).unwrap_or_default() > Duration::from_secs(days * 86_400),
        Err(_) => true,
    };
    if !due {
        return None;
    }
    let opts = Options { days, cap: u64::MAX, clean_over_cap: false, dry_run: false };
    Some(match sweep_slot(slot, &opts) {
        Ok(r) => describe(&r, 6 * GIB),
        Err(e) => format!("sweep: {}: {e:#}", slot.display()),
    })
}

/// Sweeps one slot.
pub fn sweep_slot(slot: &Path, o: &Options) -> Result<SlotReport> {
    if !slot.is_dir() {
        bail!("{} is not a directory", slot.display());
    }
    let before = dir_size(slot);
    let mut report = SlotReport {
        slot: slot.to_path_buf(),
        before,
        after: before,
        removed_entries: 0,
        method: "built-in",
        busy: false,
        cleaned: false,
        over_cap: false,
    };
    let Some(locks) = try_lock_slot(slot)? else {
        report.busy = true;
        return Ok(report);
    };
    let cutoff = SystemTime::now() - Duration::from_secs(o.days * 86_400);
    if cargo_sweep_available() && !o.dry_run {
        report.method = "cargo sweep";
        let status = Command::new("cargo")
            .args(["sweep", "--time", &o.days.to_string()])
            .arg(slot)
            .status()
            .context("running cargo sweep")?;
        if !status.success() {
            bail!("cargo sweep failed with {status}");
        }
    } else {
        report.removed_entries = sweep_older_than(slot, cutoff, o.dry_run)?;
    }
    report.after = dir_size(slot);
    report.over_cap = report.after > o.cap;
    if report.over_cap && o.clean_over_cap && !o.dry_run {
        clean_profiles(slot)?;
        report.cleaned = true;
        report.after = dir_size(slot);
    }
    if !o.dry_run {
        File::create(slot.join(STAMP)).context("writing the sweep stamp")?;
    }
    drop(locks);
    Ok(report)
}

/// Takes cargo's locks of every profile directory without waiting. `None` when a build holds one.
fn try_lock_slot(slot: &Path) -> Result<Option<Vec<File>>> {
    let mut held = Vec::new();
    for profile in profile_dirs(slot) {
        for name in CARGO_LOCKS {
            let path = profile.join(name);
            if !path.exists() {
                continue;
            }
            let f =
                File::options().read(true).write(true).open(&path).with_context(|| format!("{}", path.display()))?;
            match f.try_lock() {
                Ok(()) => held.push(f),
                Err(fs::TryLockError::WouldBlock) => return Ok(None),
                Err(fs::TryLockError::Error(e)) => {
                    return Err(e).with_context(|| format!("locking {}", path.display()));
                }
            }
        }
    }
    Ok(Some(held))
}

/// `debug/`, `release/` and `<triple>/{debug,release}/` under `slot`.
fn profile_dirs(slot: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in PROFILE_DIRS {
        let d = slot.join(p);
        if d.is_dir() {
            out.push(d);
        }
    }
    if let Ok(rd) = fs::read_dir(slot) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // A target triple directory (cross builds): x86_64-pc-windows-gnu, aarch64-apple-ios, …
            if name.matches('-').count() >= 2 && e.path().is_dir() {
                for p in PROFILE_DIRS {
                    let d = e.path().join(p);
                    if d.is_dir() {
                        out.push(d);
                    }
                }
            }
        }
    }
    out
}

fn cargo_sweep_available() -> bool {
    Command::new("cargo-sweep").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Removes every entry of the unit directories whose newest file is older than `cutoff`. Returns the count.
pub fn sweep_older_than(slot: &Path, cutoff: SystemTime, dry_run: bool) -> Result<usize> {
    let mut removed = 0;
    for profile in profile_dirs(slot) {
        for unit_dir in UNIT_DIRS {
            let dir = profile.join(unit_dir);
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let path = e.path();
                if newest_mtime(&path).is_some_and(|t| t < cutoff) {
                    removed += 1;
                    if !dry_run {
                        remove(&path).with_context(|| format!("removing {}", path.display()))?;
                    }
                }
            }
        }
    }
    Ok(removed)
}

/// Removes everything inside each profile directory except cargo's lock files.
fn clean_profiles(slot: &Path) -> Result<()> {
    for profile in profile_dirs(slot) {
        for e in fs::read_dir(&profile)?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if CARGO_LOCKS.contains(&name.as_str()) {
                continue;
            }
            remove(&e.path()).with_context(|| format!("removing {}", e.path().display()))?;
        }
    }
    Ok(())
}

fn remove(path: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

/// The newest modification time of a file, or of any file below a directory (symlinks are not followed). A
/// directory's own time only counts when it is empty: adding or removing entries bumps it without any use.
fn newest_mtime(path: &Path) -> Option<SystemTime> {
    let meta = fs::symlink_metadata(path).ok()?;
    let own = meta.modified().ok();
    if !meta.is_dir() {
        return own;
    }
    let mut newest = None;
    for e in fs::read_dir(path).ok()?.flatten() {
        if let Some(t) = newest_mtime(&e.path()) {
            newest = Some(newest.map_or(t, |n: SystemTime| n.max(t)));
        }
    }
    newest.or(own)
}

/// Total size of the files below `dir`, without following symlinks. Hard links are counted once per name.
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(dir) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    fs::read_dir(dir).map(|rd| rd.flatten().map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::fs::FileTimes;

    use super::*;

    fn touch(path: &Path, bytes: usize, age_days: u64) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, vec![0u8; bytes]).expect("write");
        let t = SystemTime::now() - Duration::from_secs(age_days * 86_400);
        File::options()
            .write(true)
            .open(path)
            .expect("open")
            .set_times(FileTimes::new().set_modified(t))
            .expect("times");
    }

    fn fake_slot() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = dir.path().join("debug");
        touch(&d.join("deps/libold-1111.rlib"), 1000, 30);
        touch(&d.join("deps/libnew-2222.rlib"), 1000, 1);
        touch(&d.join(".fingerprint/old-1111/lib-old"), 10, 30);
        touch(&d.join(".fingerprint/new-2222/lib-new"), 10, 0);
        touch(&d.join("build/mixed-3333/out/a"), 10, 30);
        touch(&d.join("build/mixed-3333/out/b"), 10, 2);
        touch(&d.join(".cargo-lock"), 0, 30);
        dir
    }

    #[test]
    fn the_built_in_sweep_removes_only_stale_units() {
        let slot = fake_slot();
        let s = slot.path();
        let cutoff = SystemTime::now() - Duration::from_secs(7 * 86_400);
        assert_eq!(sweep_older_than(s, cutoff, true).expect("dry run"), 2);
        assert!(s.join("debug/deps/libold-1111.rlib").exists(), "a dry run removes nothing");
        assert_eq!(sweep_older_than(s, cutoff, false).expect("sweep"), 2);
        assert!(!s.join("debug/deps/libold-1111.rlib").exists());
        assert!(!s.join("debug/.fingerprint/old-1111").exists());
        assert!(s.join("debug/deps/libnew-2222.rlib").exists());
        assert!(s.join("debug/.fingerprint/new-2222").exists());
        // A directory with one recent file counts as used.
        assert!(s.join("debug/build/mixed-3333/out/a").exists());
        assert!(s.join("debug/.cargo-lock").exists(), "lock files are never swept");
    }

    #[test]
    fn a_slot_over_the_cap_is_cleaned_only_on_request() {
        let slot = fake_slot();
        let s = slot.path();
        let o = Options { days: 7, cap: 100, clean_over_cap: false, dry_run: false };
        let r = sweep_slot(s, &o).expect("sweep");
        assert!(r.over_cap && !r.cleaned, "{r:?}");
        assert!(s.join(STAMP).exists());
        assert_eq!(r.removed_entries, 2);
        let o = Options { clean_over_cap: true, ..o };
        let r = sweep_slot(s, &o).expect("sweep");
        assert!(r.cleaned, "{r:?}");
        assert_eq!(r.after, 0);
        assert!(s.join("debug/.cargo-lock").exists());
        assert!(!s.join("debug/deps").exists());
    }

    #[test]
    fn a_slot_whose_lock_is_held_is_skipped() {
        let slot = fake_slot();
        let s = slot.path();
        let holder = File::options().read(true).write(true).open(s.join("debug/.cargo-lock")).expect("open");
        holder.lock().expect("lock");
        let o = Options { days: 7, cap: 0, clean_over_cap: true, dry_run: false };
        let r = sweep_slot(s, &o).expect("sweep");
        assert!(r.busy);
        assert!(s.join("debug/deps/libold-1111.rlib").exists(), "nothing is removed under a running build");
        holder.unlock().expect("unlock");
    }

    #[test]
    fn the_due_sweep_runs_once_per_period() {
        let slot = fake_slot();
        let s = slot.path();
        assert!(sweep_if_due(s, 7).is_some(), "no stamp yet: due");
        assert!(!s.join("debug/deps/libold-1111.rlib").exists());
        assert!(sweep_if_due(s, 7).is_none(), "stamped just now: not due");
        assert!(sweep_if_due(&s.join("missing"), 7).is_none());
    }

    #[test]
    fn sizes_add_up() {
        let slot = fake_slot();
        assert_eq!(dir_size(slot.path()), 1000 + 1000 + 10 + 10 + 10 + 10);
    }
}
