// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask disk-guard`: measures free space against the tiers RED < 8 GiB, AMBER 8–25, GREEN 25–45,
//! BLUE >= 45 with a 20 GiB reserve (QA §10, QA-24; PLAN §0 rule 6), before heavy work.
//!
//! - `--need N` refuses when the work would leave less than the reserve free: `free - N < reserve`. The reserve
//!   is 8 GiB (the RED line) by default and 20 GiB for `--tier blue` (QA §10: BLUE keeps a 20 GiB reserve).
//! - `--tier T` refuses below tier `T` (QA §10: the Rust workspace needs AMBER, the Flutter SDK GREEN, a local
//!   Code-OSS build BLUE).
//! - Without either it only reports. `--json` prints one object.
//!
//! The measured filesystem is the one holding `--path`, else `CARGO_TARGET_DIR` (where builds write), else
//! `$HOME` (`/home` on the dev box). `RUSSET_DISK_GUARD_FREE_GIB` simulates a free-space value for tests and
//! drills; the report then says `simulated`. The cargo wrappers (`cargo xtask ci|test|build`, [`crate::ci`]) call
//! [`check`] first. Owner: W8.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Bytes per GiB.
pub const GIB: u64 = 1024 * 1024 * 1024;

/// Simulated free space in GiB (tests and drills only).
pub const FAKE_FREE_ENV: &str = "RUSSET_DISK_GUARD_FREE_GIB";

/// Flags of `cargo xtask disk-guard`.
#[derive(Debug, Default, clap::Args)]
pub struct Args {
    /// GiB the coming work needs; refused when free space minus this falls below the reserve
    #[arg(long, value_name = "GiB")]
    pub need: Option<f64>,
    /// The lowest tier the work may run at: red, amber, green or blue
    #[arg(long, value_name = "TIER")]
    pub tier: Option<Tier>,
    /// GiB that must stay free after the work (default 8, the RED line; 20 with --tier blue)
    #[arg(long, value_name = "GiB")]
    pub reserve: Option<f64>,
    /// A path on the filesystem to measure (default: CARGO_TARGET_DIR, else $HOME)
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,
    /// Print one JSON object instead of a line of text
    #[arg(long)]
    pub json: bool,
}

/// The disk tiers of QA §10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum Tier {
    /// Below 8 GiB: edits, docs and small unit tests only.
    Red,
    /// 8–25 GiB: the Rust workspace (one cargo target slot), the extension, lab Swift builds.
    Amber,
    /// 25–45 GiB: plus the Flutter SDK; cargo target slot `b` opens.
    Green,
    /// 45 GiB and above, keeping a 20 GiB reserve: plus a local Code-OSS build.
    Blue,
}

impl Tier {
    /// The tier of `free` bytes.
    pub fn of(free: u64) -> Self {
        match free {
            f if f >= 45 * GIB => Self::Blue,
            f if f >= 25 * GIB => Self::Green,
            f if f >= 8 * GIB => Self::Amber,
            _ => Self::Red,
        }
    }

    /// The least free space of this tier, in GiB.
    pub fn floor_gib(self) -> u64 {
        match self {
            Self::Red => 0,
            Self::Amber => 8,
            Self::Green => 25,
            Self::Blue => 45,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Amber => "amber",
            Self::Green => "green",
            Self::Blue => "blue",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name().to_ascii_uppercase())
    }
}

/// What a caller asks of the disk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Request {
    pub need_gib: Option<f64>,
    pub tier: Option<Tier>,
    pub reserve_gib: Option<f64>,
    pub path: Option<PathBuf>,
}

/// The measurement and the decision.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub path: PathBuf,
    pub mount: Option<String>,
    pub free_bytes: u64,
    pub tier: Tier,
    pub simulated: bool,
    pub need_gib: Option<f64>,
    pub reserve_gib: f64,
    /// `None` when allowed; otherwise why the work is refused.
    pub refusal: Option<String>,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn free_gib(&self) -> f64 {
        self.free_bytes as f64 / GIB as f64
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "path": self.path.display().to_string(),
            "mount": self.mount,
            "freeBytes": self.free_bytes,
            "freeGib": (self.free_gib() * 10.0).round() / 10.0,
            "tier": self.tier.name(),
            "simulated": self.simulated,
            "needGib": self.need_gib,
            "reserveGib": self.reserve_gib,
            "ok": self.refusal.is_none(),
            "refusal": self.refusal,
            "warnings": self.warnings,
        })
    }

    /// One line for humans.
    pub fn line(&self) -> String {
        let mut s = format!("disk-guard: {:.1} GiB free", self.free_gib());
        if let Some(m) = &self.mount {
            s.push_str(&format!(" on {m}"));
        }
        s.push_str(&format!(" (tier {}{})", self.tier, if self.simulated { ", simulated" } else { "" }));
        if let Some(n) = self.need_gib {
            s.push_str(&format!("; need {n} GiB + {} GiB reserve", self.reserve_gib));
        }
        match &self.refusal {
            None => s.push_str(": ok"),
            Some(r) => s.push_str(&format!(": REFUSED: {r}")),
        }
        s
    }
}

pub fn run(args: Args) -> Result<()> {
    let req = Request { need_gib: args.need, tier: args.tier, reserve_gib: args.reserve, path: args.path };
    let report = check(&req)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    } else {
        for w in &report.warnings {
            println!("warning: {w}");
        }
        println!("{}", report.line());
    }
    match &report.refusal {
        None => Ok(()),
        Some(r) => bail!("disk-guard refused: {r} (QA §10; PLAN §0 rule 6)"),
    }
}

/// Measures and decides. Errors only when the measurement itself fails.
pub fn check(req: &Request) -> Result<Report> {
    check_with(req, std::env::var_os(FAKE_FREE_ENV))
}

/// [`check`] with the value of [`FAKE_FREE_ENV`] passed in.
pub fn check_with(req: &Request, fake_free_gib: Option<std::ffi::OsString>) -> Result<Report> {
    let path = match &req.path {
        Some(p) => p.clone(),
        None => default_path(),
    };
    let existing = nearest_existing(&path);
    let (free, mount, simulated) = match fake_free_bytes(fake_free_gib)? {
        Some(free) => (free, None, true),
        None => {
            let (free, mount) = measure(&existing).with_context(|| format!("measuring {}", existing.display()))?;
            (free, mount, false)
        }
    };
    let mut report = decide(free, req);
    report.path = existing;
    report.mount = mount;
    report.simulated = simulated;
    Ok(report)
}

/// The pure decision for `free` bytes (the unit-tested part).
pub fn decide(free: u64, req: &Request) -> Report {
    let tier = Tier::of(free);
    let reserve = req.reserve_gib.unwrap_or(if req.tier == Some(Tier::Blue) { 20.0 } else { 8.0 });
    let free_gib = free as f64 / GIB as f64;
    let mut refusal = None;
    if let Some(min) = req.tier
        && tier < min
    {
        refusal = Some(format!(
            "this work needs tier {min} (>= {} GiB free); {free_gib:.1} GiB free is tier {tier}",
            min.floor_gib()
        ));
    }
    if refusal.is_none()
        && let Some(need) = req.need_gib
        && free_gib - need < reserve
    {
        refusal = Some(format!(
            "needs {need} GiB and keeps a {reserve} GiB reserve, but only {free_gib:.1} GiB is free (tier {tier})"
        ));
    }
    let mut warnings = Vec::new();
    if tier == Tier::Red && refusal.is_none() {
        warnings.push(format!("tier RED ({free_gib:.1} GiB free): edits, docs and small unit tests only (QA §10)"));
    }
    if tier < Tier::Green && current_slot().as_deref() == Some("target-b") {
        warnings.push(
            "cargo target slot b is closed below tier GREEN; use slot a (PLAN §0 rule 7, ARCHITECTURE [D 81])".into(),
        );
    }
    Report {
        path: PathBuf::new(),
        mount: None,
        free_bytes: free,
        tier,
        simulated: false,
        need_gib: req.need_gib,
        reserve_gib: reserve,
        refusal,
        warnings,
    }
}

/// The file name of `CARGO_TARGET_DIR` (`target-a`, `target-b`), if set.
fn current_slot() -> Option<String> {
    let dir = std::env::var_os("CARGO_TARGET_DIR")?;
    Path::new(&dir).file_name().map(|n| n.to_string_lossy().into_owned())
}

fn default_path() -> PathBuf {
    if let Some(t) = std::env::var_os("CARGO_TARGET_DIR").filter(|t| !t.is_empty()) {
        return PathBuf::from(t);
    }
    if let Some(h) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    crate::workstreams::repo_root()
}

/// The path itself, or its nearest existing ancestor (a target slot may not exist yet).
fn nearest_existing(path: &Path) -> PathBuf {
    path.ancestors().find(|p| p.exists()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

fn fake_free_bytes(value: Option<std::ffi::OsString>) -> Result<Option<u64>> {
    let Some(v) = value else { return Ok(None) };
    let gib: f64 =
        v.to_string_lossy().trim().parse().with_context(|| format!("{FAKE_FREE_ENV}={v:?} is not a number of GiB"))?;
    anyhow::ensure!(gib >= 0.0 && gib.is_finite(), "{FAKE_FREE_ENV} must be a non-negative number of GiB");
    Ok(Some((gib * GIB as f64) as u64))
}

/// Free bytes available to this user, and the mount point.
#[cfg(unix)]
fn measure(path: &Path) -> Result<(u64, Option<String>)> {
    // POSIX output format (-P) in 1024-byte blocks (-k): one line per filesystem, even for long device names.
    let out = Command::new("df").arg("-P").arg("-k").arg(path).output().context("running df")?;
    if !out.status.success() {
        bail!("df failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_df(&String::from_utf8_lossy(&out.stdout))
}

/// Free bytes available to this user, and the drive.
#[cfg(windows)]
fn measure(path: &Path) -> Result<(u64, Option<String>)> {
    // DriveInfo.AvailableFreeSpace honours per-user quotas, like df's "Available".
    let script = "$p = Resolve-Path -LiteralPath $env:RUSSET_DG_PATH; \
                  $d = New-Object System.IO.DriveInfo ([System.IO.Path]::GetPathRoot($p.ProviderPath)); \
                  Write-Output ('{0} {1}' -f $d.AvailableFreeSpace, $d.Name)";
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("RUSSET_DG_PATH", path)
        .output()
        .context("running powershell")?;
    if !out.status.success() {
        bail!("powershell failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_drive_info(&String::from_utf8_lossy(&out.stdout))
}

/// Parses the PowerShell line `<AvailableFreeSpace> <drive name>` of the Windows measurement. Compiled and tested
/// on every platform, so only the PowerShell call itself is Windows-only.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_drive_info(text: &str) -> Result<(u64, Option<String>)> {
    let line = text.lines().map(str::trim).rfind(|l| !l.is_empty()).context("no output from DriveInfo")?;
    let mut parts = line.splitn(2, char::is_whitespace);
    let free: u64 = parts
        .next()
        .unwrap_or_default()
        .parse()
        .with_context(|| format!("DriveInfo output {line:?} does not start with a byte count"))?;
    Ok((free, parts.next().map(str::trim).filter(|d| !d.is_empty()).map(str::to_owned)))
}

/// Parses `df -P -k` output: the last line's fourth column is the available space in KiB, the sixth onwards the
/// mount point.
pub fn parse_df(text: &str) -> Result<(u64, Option<String>)> {
    let line = text.lines().rev().find(|l| !l.trim().is_empty()).context("empty df output")?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    anyhow::ensure!(fields.len() >= 6, "unexpected df line {line:?}");
    let kib: u64 = fields[3].parse().with_context(|| format!("df available column {:?}", fields[3]))?;
    Ok((kib * 1024, Some(fields[5..].join(" "))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gib(n: f64) -> u64 {
        (n * GIB as f64) as u64
    }

    #[test]
    fn tiers_follow_qa_section_10() {
        assert_eq!(Tier::of(0), Tier::Red);
        assert_eq!(Tier::of(gib(7.99)), Tier::Red);
        assert_eq!(Tier::of(gib(8.0)), Tier::Amber);
        assert_eq!(Tier::of(gib(24.9)), Tier::Amber);
        assert_eq!(Tier::of(gib(25.0)), Tier::Green);
        assert_eq!(Tier::of(gib(44.9)), Tier::Green);
        assert_eq!(Tier::of(gib(45.0)), Tier::Blue);
        assert_eq!(Tier::of(gib(98.0)), Tier::Blue);
        assert!(Tier::Red < Tier::Amber && Tier::Amber < Tier::Green && Tier::Green < Tier::Blue);
    }

    #[test]
    fn a_tier_requirement_refuses_below_it() {
        let req = Request { tier: Some(Tier::Green), ..Request::default() };
        let r = decide(gib(12.0), &req);
        assert_eq!(r.tier, Tier::Amber);
        assert!(r.refusal.as_deref().is_some_and(|m| m.contains("needs tier GREEN")), "{r:?}");
        assert!(decide(gib(30.0), &req).refusal.is_none());
    }

    #[test]
    fn need_keeps_the_reserve() {
        let req = Request { need_gib: Some(5.0), ..Request::default() };
        // 12 - 5 = 7 < 8 (the RED line): refused.
        assert!(decide(gib(12.0), &req).refusal.is_some());
        // 14 - 5 = 9 >= 8: allowed.
        assert!(decide(gib(14.0), &req).refusal.is_none());
        let custom = Request { need_gib: Some(5.0), reserve_gib: Some(2.0), ..Request::default() };
        assert!(decide(gib(8.0), &custom).refusal.is_none());
    }

    #[test]
    fn blue_work_keeps_a_20_gib_reserve() {
        let req = Request { need_gib: Some(10.0), tier: Some(Tier::Blue), ..Request::default() };
        let r = decide(gib(46.0), &req);
        assert_eq!(r.reserve_gib, 20.0);
        assert!(r.refusal.is_none(), "{r:?}");
        // Tier BLUE but 46 - 30 = 16 < 20.
        let big = Request { need_gib: Some(30.0), tier: Some(Tier::Blue), ..Request::default() };
        assert!(decide(gib(46.0), &big).refusal.as_deref().is_some_and(|m| m.contains("20 GiB reserve")));
    }

    #[test]
    fn a_bare_check_only_reports() {
        let r = decide(gib(3.0), &Request::default());
        assert!(r.refusal.is_none());
        assert_eq!(r.tier, Tier::Red);
        assert!(r.warnings.iter().any(|w| w.contains("tier RED")));
        assert_eq!(r.to_json()["ok"], serde_json::json!(true));
    }

    #[test]
    fn df_output_is_parsed() {
        let text = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
                    /dev/nvme0n1p2   495911936 414000000  80000000      84% /home\n";
        let (free, mount) = parse_df(text).expect("parse");
        assert_eq!(free, 80_000_000 * 1024);
        assert_eq!(mount.as_deref(), Some("/home"));
        let spaced = "Filesystem 1024-blocks Used Available Capacity Mounted on\nx 10 5 5 50% /mnt/a b\n";
        assert_eq!(parse_df(spaced).expect("parse").1.as_deref(), Some("/mnt/a b"));
        assert!(parse_df("").is_err());
        assert!(parse_df("Filesystem\nnonsense\n").is_err());
    }

    #[test]
    fn windows_drive_info_is_parsed() {
        let (free, drive) = parse_drive_info("85899345920 C:\\\r\n").expect("parse");
        assert_eq!(free, 80 * GIB);
        assert_eq!(drive.as_deref(), Some("C:\\"));
        // A profile or module banner before the line does not matter; the last line counts.
        assert_eq!(parse_drive_info("WARNING: x\r\n1024 D:\\\r\n").expect("parse").0, 1024);
        assert_eq!(parse_drive_info("2048").expect("parse"), (2048, None));
        assert!(parse_drive_info("").is_err());
        assert!(parse_drive_info("C:\\ 1024").is_err());
    }

    #[test]
    fn the_real_filesystem_can_be_measured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (free, _) = measure(dir.path()).expect("measure");
        assert!(free > 0);
    }

    /// PLAN W8 test: "disk-guard refuses below a fake tier". The simulated free space goes through the same
    /// path as `RUSSET_DISK_GUARD_FREE_GIB`, so the test never depends on the real disk.
    #[test]
    fn the_guard_refuses_below_a_simulated_tier() {
        let req = Request { tier: Some(Tier::Amber), need_gib: Some(2.0), ..Request::default() };
        let refused = check_with(&req, Some("5".into())).expect("check");
        assert!(refused.simulated);
        assert_eq!(refused.tier, Tier::Red);
        assert!(refused.refusal.is_some());
        assert!(refused.line().contains("REFUSED") && refused.line().contains("simulated"), "{}", refused.line());
        let allowed = check_with(&req, Some("60".into())).expect("check");
        assert!(allowed.line().ends_with(": ok"), "{}", allowed.line());
        assert!(check_with(&req, Some("lots".into())).is_err());
        assert!(check_with(&req, Some("-1".into())).is_err());
    }
}
