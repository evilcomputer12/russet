// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask ci-budget`: the month's GitHub Actions use against the included minutes, with a warning at 80 %
//! (QA §4.8; ARCHITECTURE [D 77]; PLAN W8).
//!
//! The private repository runs on GitHub Free with no payment method: 2,000 included minutes a month, consumed at
//! list price, so a Windows minute costs about 1.67 Linux minutes and a macOS minute about 10.3 (QA V16). The
//! command reports *Linux-equivalent* minutes:
//!
//! 1. from the account's billing usage report (`gh api /users/<owner>/settings/billing/usage`), when the token
//!    has the `user` scope: the gross amount of the `actions` product divided by the Linux per-minute price;
//! 2. otherwise from the repository's workflow runs of the month and their jobs (`repo` scope is enough): each
//!    job's duration rounded up to a whole minute, as GitHub bills it, times the runner's price ratio.
//!
//! A public repository pays nothing on standard runners, and the command says so. Larger runners (`-large`,
//! `-xlarge`) never use included minutes; they are counted apart. `--from-dir` reads recorded API answers
//! (`runs.json`, `jobs-<run id>.json`) for tests and offline review. It only reads: no API call changes
//! anything. Owner: W8.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// List prices per minute in USD (QA V16, 2026-10-11): standard 2-core Linux, Windows and macOS runners.
const PRICE_LINUX: f64 = 0.006;
const PRICE_WINDOWS: f64 = 0.010;
const PRICE_MACOS: f64 = 0.062;

/// Flags of `cargo xtask ci-budget`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The repository (default: the `origin` remote)
    #[arg(long, value_name = "OWNER/NAME")]
    pub repo: Option<String>,
    /// The month to report (default: the current UTC month)
    #[arg(long, value_name = "YYYY-MM")]
    pub month: Option<String>,
    /// Included Linux minutes per month (GitHub Free: 2,000; Pro: 3,000)
    #[arg(long, default_value_t = 2000.0)]
    pub included: f64,
    /// Warn at this share of the included minutes, in percent
    #[arg(long, default_value_t = 80.0)]
    pub warn_percent: f64,
    /// Exit non-zero at or above the warning threshold
    #[arg(long)]
    pub strict: bool,
    /// Read recorded API answers (`runs.json`, `jobs-<id>.json`) instead of calling gh
    #[arg(long, value_name = "DIR")]
    pub from_dir: Option<PathBuf>,
    /// Print one JSON object
    #[arg(long)]
    pub json: bool,
}

/// The runner families GitHub prices differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Runner {
    Linux,
    Windows,
    MacOs,
    /// Self-hosted runners use no GitHub minutes.
    SelfHosted,
    /// Larger runners never use included minutes and are never free.
    Larger,
}

impl Runner {
    /// The family of a job's `runs-on` labels.
    pub fn of(labels: &[String]) -> Self {
        let l: Vec<String> = labels.iter().map(|s| s.to_ascii_lowercase()).collect();
        if l.iter().any(|s| s == "self-hosted") {
            Self::SelfHosted
        } else if l.iter().any(|s| s.ends_with("-large") || s.ends_with("-xlarge") || s.contains("-core")) {
            Self::Larger
        } else if l.iter().any(|s| s.starts_with("windows")) {
            Self::Windows
        } else if l.iter().any(|s| s.starts_with("macos") || s.starts_with("xcode")) {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// Linux minutes one minute of this runner consumes from the included quota.
    pub fn ratio(self) -> f64 {
        match self {
            Self::Linux => 1.0,
            Self::Windows => PRICE_WINDOWS / PRICE_LINUX,
            Self::MacOs => PRICE_MACOS / PRICE_LINUX,
            Self::SelfHosted | Self::Larger => 0.0,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::MacOs => "macos",
            Self::SelfHosted => "self-hosted",
            Self::Larger => "larger",
        }
    }
}

/// The month's use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    /// Linux-equivalent minutes taken from the included quota.
    pub linux_equivalent: f64,
    /// Billed (rounded) minutes per runner family.
    pub minutes: BTreeMap<&'static str, u64>,
    pub runs: usize,
    pub jobs: usize,
    pub source: &'static str,
}

pub fn run(args: Args) -> Result<()> {
    let (year, month) = match &args.month {
        Some(m) => parse_month(m)?,
        None => current_month(),
    };
    let usage = match &args.from_dir {
        Some(dir) => usage_from_runs(&recorded_runs(dir)?, |id| recorded_jobs(dir, id), year, month)?,
        None => {
            let repo = match &args.repo {
                Some(r) => r.clone(),
                None => origin_repo()?,
            };
            let info = gh_json(&format!("repos/{repo}")).context("reading the repository through gh")?;
            if info["private"] == Value::Bool(false) {
                println!("ci-budget: {repo} is public: standard GitHub-hosted runners are free (QA §4.8)");
                return Ok(());
            }
            let owner = repo.split('/').next().unwrap_or_default().to_string();
            match billing_usage(&owner, year, month) {
                Some(u) => u,
                None => {
                    let runs = list_runs(&repo, year, month)?;
                    usage_from_runs(
                        &runs,
                        |id| gh_json(&format!("repos/{repo}/actions/runs/{id}/jobs?per_page=100&filter=all")),
                        year,
                        month,
                    )?
                }
            }
        }
    };
    let percent = 100.0 * usage.linux_equivalent / args.included;
    let over = percent >= args.warn_percent;
    if args.json {
        let out = serde_json::json!({
            "month": format!("{year:04}-{month:02}"),
            "linuxEquivalentMinutes": (usage.linux_equivalent * 10.0).round() / 10.0,
            "includedMinutes": args.included,
            "percent": (percent * 10.0).round() / 10.0,
            "warnPercent": args.warn_percent,
            "warning": over,
            "minutesByRunner": usage.minutes,
            "runs": usage.runs,
            "jobs": usage.jobs,
            "source": usage.source,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        let by: Vec<String> = usage.minutes.iter().map(|(k, v)| format!("{k} {v} min")).collect();
        println!(
            "ci-budget {year:04}-{month:02}: {:.0} of {:.0} included minutes ({percent:.1} %), Linux-equivalent; \
             {} run(s), {} job(s){}; source: {}",
            usage.linux_equivalent,
            args.included,
            usage.runs,
            usage.jobs,
            if by.is_empty() { String::new() } else { format!(" ({})", by.join(", ")) },
            usage.source
        );
        if usage.minutes.get(Runner::Larger.name()).is_some_and(|m| *m > 0) {
            println!("warning: larger runners ran; they never use included minutes (QA §4.2: never use them)");
        }
        if over {
            println!(
                "warning: {percent:.0} % of the included minutes used (threshold {:.0} %): batch merges into fewer \
                 main pushes; the lead's local `cargo xtask ci --profile full` stays the merge gate (QA §4.8)",
                args.warn_percent
            );
        }
    }
    if over && args.strict {
        bail!("ci-budget: {percent:.1} % of the included minutes used (--strict)");
    }
    Ok(())
}

/// `OWNER/NAME` from the `origin` remote (https or ssh form).
fn origin_repo() -> Result<String> {
    let root = crate::workstreams::repo_root();
    let out = Command::new("git").arg("-C").arg(&root).args(["remote", "get-url", "origin"]).output()?;
    if !out.status.success() {
        bail!("no origin remote: pass --repo OWNER/NAME");
    }
    parse_remote(String::from_utf8_lossy(&out.stdout).trim()).context("cannot read OWNER/NAME from the origin remote")
}

/// `https://github.com/o/r(.git)` or `git@github.com:o/r(.git)` → `o/r`.
pub fn parse_remote(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = rest.split('/');
    let (o, r) = (parts.next()?, parts.next()?);
    (!o.is_empty() && !r.is_empty() && parts.next().is_none()).then(|| format!("{o}/{r}"))
}

fn gh_json(path: &str) -> Result<Value> {
    let out = Command::new("gh")
        .args(["api", "-H", "Accept: application/vnd.github+json", path])
        .output()
        .context("running gh")?;
    if !out.status.success() {
        bail!("gh api {path}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("gh api {path}: not JSON"))
}

/// The billing usage report, when the token may read it (`user` scope); `None` otherwise.
fn billing_usage(owner: &str, year: i64, month: u32) -> Option<Usage> {
    let v = gh_json(&format!("users/{owner}/settings/billing/usage?year={year}&month={month}")).ok()?;
    usage_from_billing(&v)
}

/// Linux-equivalent minutes from a billing usage report: the `actions` gross amount over the Linux price.
pub fn usage_from_billing(v: &Value) -> Option<Usage> {
    let items = v.get("usageItems")?.as_array()?;
    let mut u = Usage { source: "billing usage report", ..Usage::default() };
    for it in items {
        if !it["product"].as_str().is_some_and(|p| p.eq_ignore_ascii_case("actions")) {
            continue;
        }
        let unit = it["unitType"].as_str().unwrap_or_default().to_ascii_lowercase();
        if !unit.starts_with("minute") {
            continue;
        }
        u.linux_equivalent += it["grossAmount"].as_f64().unwrap_or(0.0) / PRICE_LINUX;
        let sku = it["sku"].as_str().unwrap_or_default().to_ascii_lowercase();
        let family = if sku.contains("windows") {
            Runner::Windows
        } else if sku.contains("macos") {
            Runner::MacOs
        } else {
            Runner::Linux
        };
        *u.minutes.entry(family.name()).or_default() += it["quantity"].as_f64().unwrap_or(0.0).round() as u64;
    }
    Some(u)
}

/// The month's runs, newest first (every page).
fn list_runs(repo: &str, year: i64, month: u32) -> Result<Vec<Value>> {
    let (first, last) = month_days(year, month);
    let mut all = Vec::new();
    for page in 1..=50 {
        let v = gh_json(&format!("repos/{repo}/actions/runs?per_page=100&page={page}&created={first}..{last}"))?;
        let runs = v["workflow_runs"].as_array().cloned().unwrap_or_default();
        let n = runs.len();
        all.extend(runs);
        if n < 100 {
            break;
        }
    }
    Ok(all)
}

fn recorded_runs(dir: &Path) -> Result<Vec<Value>> {
    let text = std::fs::read_to_string(dir.join("runs.json")).context("reading runs.json")?;
    let v: Value = serde_json::from_str(&text).context("runs.json")?;
    Ok(match v {
        Value::Array(a) => a,
        other => other["workflow_runs"].as_array().cloned().unwrap_or_default(),
    })
}

fn recorded_jobs(dir: &Path, id: u64) -> Result<Value> {
    let text =
        std::fs::read_to_string(dir.join(format!("jobs-{id}.json"))).with_context(|| format!("jobs-{id}.json"))?;
    serde_json::from_str(&text).with_context(|| format!("jobs-{id}.json"))
}

/// Linux-equivalent minutes from the runs of the month and their jobs.
pub fn usage_from_runs(
    runs: &[Value],
    mut jobs_of: impl FnMut(u64) -> Result<Value>,
    year: i64,
    month: u32,
) -> Result<Usage> {
    let mut u = Usage { source: "workflow runs and jobs", ..Usage::default() };
    for run in runs {
        let Some(id) = run["id"].as_u64() else { continue };
        if let Some(created) = run["created_at"].as_str().and_then(parse_rfc3339)
            && month_of(created) != (year, month)
        {
            continue;
        }
        u.runs += 1;
        let jobs = jobs_of(id)?;
        for job in jobs["jobs"].as_array().into_iter().flatten() {
            let (Some(start), Some(end)) = (
                job["started_at"].as_str().and_then(parse_rfc3339),
                job["completed_at"].as_str().and_then(parse_rfc3339),
            ) else {
                continue;
            };
            let labels: Vec<String> =
                job["labels"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(str::to_owned)).collect();
            let runner = Runner::of(&labels);
            let secs = (end - start).max(0);
            // GitHub rounds each job up to the next whole minute; a job that never ran bills nothing.
            let minutes = if secs == 0 { 0 } else { (secs as u64).div_ceil(60) };
            u.jobs += 1;
            *u.minutes.entry(runner.name()).or_default() += minutes;
            u.linux_equivalent += minutes as f64 * runner.ratio();
        }
    }
    Ok(u)
}

// ---- dates (UTC, proleptic Gregorian; no time crate in xtask) -------------------------------------------

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Seconds since the epoch of an RFC 3339 timestamp (`2026-10-11T09:00:00Z`, fractions and offsets allowed).
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        rest = &frac[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        o if o.len() == 6 && (o.starts_with('+') || o.starts_with('-')) && &o[3..4] == ":" => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            sign * (o[1..3].parse::<i64>().ok()? * 3600 + o[4..6].parse::<i64>().ok()? * 60)
        }
        _ => return None,
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + se - offset)
}

fn month_of(epoch_secs: i64) -> (i64, u32) {
    let (y, m, _) = civil_from_days(epoch_secs.div_euclid(86_400));
    (y, m)
}

fn current_month() -> (i64, u32) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    month_of(secs)
}

fn parse_month(s: &str) -> Result<(i64, u32)> {
    let (y, m) = s.split_once('-').context("--month is YYYY-MM")?;
    let (y, m): (i64, u32) = (y.parse().context("year")?, m.parse().context("month")?);
    anyhow::ensure!((1..=12).contains(&m) && (2000..=9999).contains(&y), "--month {s} is out of range");
    Ok((y, m))
}

/// The first and last day of a month, `YYYY-MM-DD`.
fn month_days(y: i64, m: u32) -> (String, String) {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let (ly, lm, ld) = civil_from_days(days_from_civil(ny, nm, 1) - 1);
    (format!("{y:04}-{m:02}-01"), format!("{ly:04}-{lm:02}-{ld:02}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn dates_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(days_from_civil(2026, 10, 11)), (2026, 10, 11));
        assert_eq!(civil_from_days(days_from_civil(2028, 2, 29)), (2028, 2, 29));
        assert_eq!(month_days(2026, 2), ("2026-02-01".into(), "2026-02-28".into()));
        assert_eq!(month_days(2028, 2).1, "2028-02-29");
        assert_eq!(month_days(2026, 12), ("2026-12-01".into(), "2026-12-31".into()));
        assert_eq!(parse_rfc3339("1970-01-01T00:01:00Z"), Some(60));
        assert_eq!(parse_rfc3339("2026-10-11T09:00:00.123Z"), parse_rfc3339("2026-10-11T11:00:00+02:00"));
        assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("yesterday"), None);
        assert_eq!(parse_month("2026-10").expect("month"), (2026, 10));
        assert!(parse_month("2026-13").is_err());
    }

    #[test]
    fn runners_are_priced_like_github() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(Runner::of(&l(&["ubuntu-24.04"])), Runner::Linux);
        assert_eq!(Runner::of(&l(&["windows-2025"])), Runner::Windows);
        assert_eq!(Runner::of(&l(&["xcode-27"])), Runner::MacOs);
        assert_eq!(Runner::of(&l(&["macos-26"])), Runner::MacOs);
        assert_eq!(Runner::of(&l(&["self-hosted", "linux"])), Runner::SelfHosted);
        assert_eq!(Runner::of(&l(&["xcode-27-xlarge"])), Runner::Larger);
        assert!((Runner::Windows.ratio() - 1.6667).abs() < 0.001);
        assert!((Runner::MacOs.ratio() - 10.333).abs() < 0.001);
    }

    #[test]
    fn jobs_are_rounded_up_and_weighted() {
        let runs = vec![
            json!({"id": 1, "created_at": "2026-10-02T10:00:00Z"}),
            json!({"id": 2, "created_at": "2026-10-03T10:00:00Z"}),
            json!({"id": 3, "created_at": "2026-09-30T23:59:59Z"}),
        ];
        let jobs = |id: u64| -> Result<Value> {
            Ok(match id {
                1 => json!({"jobs": [
                    {"started_at": "2026-10-02T10:00:00Z", "completed_at": "2026-10-02T10:12:01Z", "labels": ["ubuntu-24.04"]},
                    {"started_at": "2026-10-02T10:00:00Z", "completed_at": "2026-10-02T10:00:00Z", "labels": ["ubuntu-24.04"]},
                ]}),
                2 => json!({"jobs": [
                    {"started_at": "2026-10-03T10:00:00Z", "completed_at": "2026-10-03T10:20:00Z", "labels": ["windows-2025"]},
                    {"started_at": "2026-10-03T10:00:00Z", "completed_at": null, "labels": ["ubuntu-24.04"]},
                ]}),
                _ => panic!("run {id} is outside the month and must not be fetched"),
            })
        };
        let u = usage_from_runs(&runs, jobs, 2026, 10).expect("usage");
        assert_eq!(u.runs, 2);
        assert_eq!(u.minutes["linux"], 13, "12 min 1 s bills 13 minutes; a skipped job bills nothing");
        assert_eq!(u.minutes["windows"], 20);
        assert!((u.linux_equivalent - (13.0 + 20.0 * PRICE_WINDOWS / PRICE_LINUX)).abs() < 1e-9);
    }

    #[test]
    fn the_billing_report_counts_gross_amounts() {
        let v = json!({"usageItems": [
            {"product": "Actions", "sku": "Actions Linux", "unitType": "Minutes", "quantity": 1200.0, "grossAmount": 7.2},
            {"product": "Actions", "sku": "Actions Windows", "unitType": "Minutes", "quantity": 80.0, "grossAmount": 0.8},
            {"product": "Actions", "sku": "Actions storage", "unitType": "GigabyteHours", "quantity": 3.0, "grossAmount": 0.01},
            {"product": "Packages", "sku": "Packages storage", "unitType": "GigabyteHours", "quantity": 1.0, "grossAmount": 1.0},
        ]});
        let u = usage_from_billing(&v).expect("usage");
        assert!((u.linux_equivalent - (1200.0 + 80.0 * PRICE_WINDOWS / PRICE_LINUX)).abs() < 1e-6, "{u:?}");
        assert_eq!(u.minutes["windows"], 80);
        assert!(usage_from_billing(&json!({"message": "Not Found"})).is_none());
    }

    #[test]
    fn remotes_name_the_repository() {
        assert_eq!(
            parse_remote("https://github.com/evilcomputer12/russet.git").as_deref(),
            Some("evilcomputer12/russet")
        );
        assert_eq!(parse_remote("git@github.com:evilcomputer12/russet").as_deref(), Some("evilcomputer12/russet"));
        assert_eq!(parse_remote("https://example.com/a/b"), None);
    }

    #[test]
    fn recorded_answers_drive_the_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("runs.json"),
            r#"{"workflow_runs": [{"id": 7, "created_at": "2026-10-05T01:00:00Z"}]}"#,
        )
        .expect("write");
        std::fs::write(
            dir.path().join("jobs-7.json"),
            r#"{"jobs": [{"started_at": "2026-10-05T01:00:00Z", "completed_at": "2026-10-05T01:30:00Z", "labels": ["ubuntu-24.04"]}]}"#,
        )
        .expect("write");
        let args = |strict| Args {
            repo: None,
            month: Some("2026-10".into()),
            included: 35.0,
            warn_percent: 80.0,
            strict,
            from_dir: Some(dir.path().to_path_buf()),
            json: false,
        };
        // 30 of 35 minutes is 86 %: a warning, and a failure only with --strict.
        run(args(false)).expect("warns only");
        assert!(run(args(true)).is_err());
    }
}
