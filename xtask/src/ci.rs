// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask ci`, `cargo xtask test` and `cargo xtask build`: the CI gates locally and the cargo wrappers
//! that check the disk first, with `CARGO_INCREMENTAL=0` (PLAN §0 rules 7 and 10; QA §4.9) [D 81].
//!
//! - `ci --profile pr` (the Definition of done): disk-guard, fmt, clippy `-D warnings`, nextest (or `cargo test`
//!   when nextest is missing), `cargo deny check licenses bans sources`, `guard all`, check-layers, check-paths
//!   (warnings only, on a `ws/…` branch) and schema drift (`cargo xtask schema --check`, skipped while W2's
//!   module is a stub).
//! - `ci --profile full` (before the lead merges): `pr` plus clippy with every feature, doc tests, rustdoc
//!   `-D warnings` and `cargo deny check` with advisories (network).
//! - `ci --profile windows`: build and the unit tests (L1) of every crate but xtask (a running xtask.exe cannot
//!   be relinked on Windows).
//! - `ci --quick`: fmt, plus clippy and the unit tests of the crates changed against `--base`.
//!
//! A missing optional tool (cargo-nextest, cargo-deny, and the guard's reuse, typos, gitleaks, zizmor) never
//! stops a local run: the step is skipped or falls back, and the summary names what is missing and how to
//! install it. `--require-tools` (CI) turns a missing tool into a failure. Every wrapper also runs the weekly
//! target-slot sweep when it is due ([`crate::sweep::sweep_if_due`]). Owner: W8.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::disk_guard::{self, Tier};
use crate::workstreams;

/// The free space a workspace build or test run needs on top of the reserve (QA §10: AMBER suffices).
const NEED_GIB: f64 = 2.0;
/// Sweep a slot when its last sweep is older than this many days (PLAN §0 rule 7: weekly).
const SWEEP_DAYS: u64 = 7;
/// The owner's one-time install of the QA tools (QA Appendix A, plus cargo-sweep).
pub const PACMAN_LINE: &str = "sudo pacman -S --needed cargo-nextest cargo-deny cargo-about cargo-llvm-cov git-cliff \
     reuse typos zizmor gitleaks mdbook lychee minisign xorg-server-xvfb cargo-insta cargo-fuzz cargo-machete \
     cargo-auditable msitools cargo-sweep";

/// The profiles of `cargo xtask ci`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Profile {
    /// The Definition of done (PLAN §0 rule 10)
    Pr,
    /// Before a merge: pr plus every feature, doc tests, rustdoc and advisories
    Full,
    /// The weekly Windows job: build and unit tests
    Windows,
}

/// Flags of `cargo xtask ci`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Which gates to run
    #[arg(long, value_enum, default_value_t = Profile::Pr)]
    pub profile: Profile,
    /// Inner loop: fmt, plus clippy and unit tests of the crates changed against --base
    #[arg(long)]
    pub quick: bool,
    /// The branch --quick and check-paths compare against
    #[arg(long, default_value = "main")]
    pub base: String,
    /// The base of the guard's commit check (default: the merge base with main); CI passes the pushed range
    #[arg(long, value_name = "REF")]
    pub guard_base: Option<String>,
    /// Stop at the first failing step
    #[arg(long)]
    pub fail_fast: bool,
    /// Fail when an optional tool is missing instead of skipping its step (CI installs every tool)
    #[arg(long)]
    pub require_tools: bool,
    /// Skip a step by name (repeatable)
    #[arg(long = "skip", value_name = "STEP")]
    pub skip: Vec<String>,
    /// Print the steps without running them
    #[arg(long)]
    pub dry_run: bool,
}

/// Flags of `cargo xtask test`: everything after `test` goes to `cargo nextest run` (or `cargo test`).
#[derive(Debug, clap::Args)]
pub struct TestArgs {
    /// Arguments for cargo nextest run (cargo test when nextest is missing)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "ARGS")]
    pub rest: Vec<String>,
}

/// Flags of `cargo xtask build`: everything after `build` goes to `cargo build`.
#[derive(Debug, clap::Args)]
pub struct BuildArgs {
    /// Arguments for cargo build
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "ARGS")]
    pub rest: Vec<String>,
}

/// How a step ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok(String),
    Failed(String),
    Skipped(String),
}

/// One gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// disk-guard at tier AMBER with [`NEED_GIB`]; also runs the due sweep.
    DiskGuard,
    /// `cargo <args>`.
    Cargo(Vec<String>),
    /// `cargo nextest run <nextest>` or, without nextest, `cargo test <fallback>`.
    Tests { nextest: Vec<String>, fallback: Vec<String> },
    /// `cargo deny <args>`, skipped without cargo-deny.
    Deny(Vec<String>),
    /// `cargo xtask guard all` (in-process).
    Guard,
    /// `cargo xtask check-layers` (in-process).
    CheckLayers,
    /// `cargo xtask check-paths` against the base (in-process; only on a `ws/…` branch).
    CheckPaths,
    /// `cargo xtask schema --check` (skipped while the module is a stub).
    Schema,
}

/// A named gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub name: &'static str,
    pub action: Action,
    /// Extra environment for this step.
    pub env: Vec<(&'static str, &'static str)>,
}

fn step(name: &'static str, action: Action) -> Step {
    Step { name, action, env: Vec::new() }
}

fn cargo(args: &[&str]) -> Action {
    Action::Cargo(args.iter().map(|s| s.to_string()).collect())
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// The steps of a profile.
pub fn plan(profile: Profile) -> Vec<Step> {
    let mut steps = vec![step("disk-guard", Action::DiskGuard)];
    match profile {
        Profile::Pr | Profile::Full => {
            steps.push(step("fmt", cargo(&["fmt", "--all", "--", "--check"])));
            steps.push(step(
                "clippy",
                cargo(&["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"]),
            ));
            if profile == Profile::Full {
                steps.push(step(
                    "clippy-all-features",
                    cargo(&[
                        "clippy",
                        "--workspace",
                        "--all-targets",
                        "--all-features",
                        "--locked",
                        "--",
                        "-D",
                        "warnings",
                    ]),
                ));
            }
            steps.push(step(
                "test",
                Action::Tests {
                    nextest: strings(&["--workspace", "--locked", "--profile", "ci"]),
                    fallback: strings(&["--workspace", "--locked"]),
                },
            ));
            if profile == Profile::Full {
                steps.push(step("doctest", cargo(&["test", "--workspace", "--locked", "--doc"])));
                let mut doc = step("doc", cargo(&["doc", "--workspace", "--no-deps", "--locked"]));
                doc.env.push(("RUSTDOCFLAGS", "-D warnings"));
                steps.push(doc);
                steps.push(step("deny", Action::Deny(strings(&["check"]))));
            } else {
                steps.push(step("deny", Action::Deny(strings(&["check", "licenses", "bans", "sources"]))));
            }
            steps.push(step("guard", Action::Guard));
            steps.push(step("check-layers", Action::CheckLayers));
            steps.push(step("check-paths", Action::CheckPaths));
            steps.push(step("schema", Action::Schema));
        }
        Profile::Windows => {
            steps.push(step(
                "build",
                cargo(&["build", "--workspace", "--exclude", "xtask", "--all-targets", "--locked"]),
            ));
            steps.push(step(
                "test-unit",
                Action::Tests {
                    nextest: strings(&["--workspace", "--exclude", "xtask", "--lib", "--locked", "--profile", "ci"]),
                    fallback: strings(&["--workspace", "--exclude", "xtask", "--lib", "--locked"]),
                },
            ));
        }
    }
    steps
}

/// The steps of `--quick` for the changed packages.
pub fn quick_plan(changed: &BTreeSet<String>) -> Vec<Step> {
    let mut steps = vec![step("disk-guard", Action::DiskGuard), step("fmt", cargo(&["fmt", "--all", "--", "--check"]))];
    if changed.is_empty() {
        return steps;
    }
    let mut clippy = strings(&["clippy", "--all-targets", "--locked"]);
    let mut tests = strings(&["--lib", "--locked"]);
    for p in changed {
        clippy.extend(["-p".to_string(), p.clone()]);
        tests.extend(["-p".to_string(), p.clone()]);
    }
    clippy.extend(strings(&["--", "-D", "warnings"]));
    steps.push(step("clippy", Action::Cargo(clippy)));
    steps.push(step("test", Action::Tests { nextest: tests.clone(), fallback: tests }));
    steps
}

pub fn run(args: Args) -> Result<()> {
    let root = workstreams::repo_root();
    let (title, steps) = if args.quick {
        let changed = changed_packages(&root, &args.base)?;
        let shown = if changed.is_empty() {
            "none".to_string()
        } else {
            changed.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        println!("ci --quick: packages changed against {}: {shown}", args.base);
        ("ci --quick".to_string(), quick_plan(&changed))
    } else {
        (format!("ci --profile {}", profile_name(args.profile)), plan(args.profile))
    };
    let steps: Vec<Step> = steps.into_iter().filter(|s| !args.skip.iter().any(|k| k == s.name)).collect();
    if args.dry_run {
        println!("{title}: {} step(s)", steps.len());
        for (i, s) in steps.iter().enumerate() {
            println!("  {:>2}. {:<20} {}", i + 1, s.name, describe(&s.action));
        }
        return Ok(());
    }
    let mut results: Vec<(&'static str, Outcome, Duration)> = Vec::new();
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for (i, s) in steps.iter().enumerate() {
        println!("\n==> [{}/{}] {}: {}", i + 1, steps.len(), s.name, describe(&s.action));
        let start = Instant::now();
        let outcome = run_step(&root, s, &args, &mut missing);
        let failed = matches!(outcome, Outcome::Failed(_));
        results.push((s.name, outcome, start.elapsed()));
        if failed && args.fail_fast {
            break;
        }
    }
    println!("\n{title} summary:");
    for (name, outcome, took) in &results {
        let (label, note) = match outcome {
            Outcome::Ok(n) => ("ok", n),
            Outcome::Failed(n) => ("FAILED", n),
            Outcome::Skipped(n) => ("skipped", n),
        };
        println!("  {label:<8} {name:<20} {:>7.1}s  {note}", took.as_secs_f64());
    }
    if !missing.is_empty() {
        println!("missing optional tools: {}", missing.iter().cloned().collect::<Vec<_>>().join(", "));
        println!("install them once with: {PACMAN_LINE}");
    }
    let failed: Vec<&str> =
        results.iter().filter(|(_, o, _)| matches!(o, Outcome::Failed(_))).map(|(n, _, _)| *n).collect();
    if !failed.is_empty() {
        bail!("{title}: {} step(s) failed: {}", failed.len(), failed.join(", "));
    }
    if args.require_tools && !missing.is_empty() {
        bail!("{title}: --require-tools and {} tool(s) missing", missing.len());
    }
    println!("{title}: green");
    Ok(())
}

fn profile_name(p: Profile) -> &'static str {
    match p {
        Profile::Pr => "pr",
        Profile::Full => "full",
        Profile::Windows => "windows",
    }
}

fn describe(a: &Action) -> String {
    match a {
        Action::DiskGuard => format!("disk-guard --tier amber --need {NEED_GIB}"),
        Action::Cargo(args) => format!("cargo {}", args.join(" ")),
        Action::Tests { nextest, .. } => {
            format!("cargo nextest run {} (cargo test without nextest)", nextest.join(" "))
        }
        Action::Deny(args) => format!("cargo deny {}", args.join(" ")),
        Action::Guard => "cargo xtask guard all".into(),
        Action::CheckLayers => "cargo xtask check-layers".into(),
        Action::CheckPaths => "cargo xtask check-paths (ws/… branches; warnings only)".into(),
        Action::Schema => "cargo xtask schema --check".into(),
    }
}

fn run_step(root: &Path, s: &Step, args: &Args, missing: &mut BTreeSet<String>) -> Outcome {
    match &s.action {
        Action::DiskGuard => match preflight() {
            Ok(line) => Outcome::Ok(line),
            Err(e) => Outcome::Failed(format!("{e:#}")),
        },
        Action::Cargo(a) => status_outcome(cargo_status(root, a, &s.env)),
        Action::Tests { nextest, fallback } => {
            if has_cargo_subcommand("nextest") {
                let mut a = vec!["nextest".to_string(), "run".to_string()];
                a.extend(nextest.iter().cloned());
                status_outcome(cargo_status(root, &a, &s.env))
            } else {
                missing.insert("cargo-nextest".into());
                println!("cargo-nextest is not installed: falling back to cargo test");
                let mut a = vec!["test".to_string()];
                a.extend(fallback.iter().cloned());
                match status_outcome(cargo_status(root, &a, &s.env)) {
                    Outcome::Ok(_) => Outcome::Ok("cargo test (cargo-nextest missing)".into()),
                    other => other,
                }
            }
        }
        Action::Deny(a) => {
            if !has_cargo_subcommand("deny") {
                missing.insert("cargo-deny".into());
                return Outcome::Skipped("cargo-deny is not installed".into());
            }
            let mut full = vec!["deny".to_string()];
            full.extend(a.iter().cloned());
            status_outcome(cargo_status(root, &full, &s.env))
        }
        Action::Guard => {
            let opts = crate::guard::Options {
                require_tools: args.require_tools,
                base: args.guard_base.clone(),
                ..crate::guard::Options::default()
            };
            match crate::guard::run_checks(root, &[crate::guard::Check::All], &opts) {
                Ok(summary) => {
                    missing.extend(summary.missing_tools.iter().cloned());
                    if summary.failed.is_empty() {
                        Outcome::Ok(summary.note())
                    } else {
                        Outcome::Failed(format!("guard: {}", summary.failed.join(", ")))
                    }
                }
                Err(e) => Outcome::Failed(format!("{e:#}")),
            }
        }
        Action::CheckLayers => match crate::layers::run(crate::layers::Args { verbose: false }) {
            Ok(()) => Outcome::Ok(String::new()),
            Err(e) => Outcome::Failed(format!("{e:#}")),
        },
        Action::CheckPaths => {
            // In a pull request's CI run HEAD is detached; GitHub names the head branch.
            let branch = std::env::var("GITHUB_HEAD_REF")
                .ok()
                .filter(|b| !b.is_empty())
                .or_else(|| git_stdout(root, &["rev-parse", "--abbrev-ref", "HEAD"]))
                .unwrap_or_default();
            let branch = branch.trim();
            let ws = match crate::paths::normalize_ws(branch) {
                Ok(ws) if branch.starts_with("ws/") => ws,
                _ => return Outcome::Skipped(format!("not on a ws/… branch ({branch})")),
            };
            let base = if git_ok(root, &["rev-parse", "--verify", "--quiet", &args.base]) {
                args.base.clone()
            } else {
                format!("origin/{}", args.base)
            };
            let a = crate::paths::Args { ws: Some(ws), base, paths: Vec::new(), strict: false };
            match crate::paths::run(a) {
                Ok(()) => Outcome::Ok("warnings only".into()),
                Err(e) => Outcome::Failed(format!("{e:#}")),
            }
        }
        Action::Schema => schema_drift(root),
    }
}

/// disk-guard at tier AMBER, then the due sweep of the current slot.
fn preflight() -> Result<String> {
    let req = disk_guard::Request { need_gib: Some(NEED_GIB), tier: Some(Tier::Amber), ..Default::default() };
    // A filesystem that cannot be measured (an unusual mount, a Windows runner without PowerShell) is not a full
    // one: warn and go on. `cargo xtask disk-guard` itself still fails.
    let report = match disk_guard::check(&req) {
        Ok(r) => r,
        Err(e) => {
            println!("warning: disk-guard could not measure free space ({e:#}); continuing");
            return Ok("free space not measured".into());
        }
    };
    for w in &report.warnings {
        println!("warning: {w}");
    }
    println!("{}", report.line());
    if let Some(r) = &report.refusal {
        bail!("disk-guard refused: {r}");
    }
    if let Some(slot) = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from)
        && let Some(note) = crate::sweep::sweep_if_due(&slot, SWEEP_DAYS)
    {
        println!("{note}");
    }
    Ok(format!("{:.1} GiB free (tier {})", report.free_gib(), report.tier))
}

fn status_outcome(r: Result<ExitStatus>) -> Outcome {
    match r {
        Ok(s) if s.success() => Outcome::Ok(String::new()),
        Ok(s) => Outcome::Failed(format!("exited with {s}")),
        Err(e) => Outcome::Failed(format!("{e:#}")),
    }
}

/// Runs cargo in the repository root (so `.cargo/config.toml` and its worktree guard apply), with
/// `CARGO_INCREMENTAL=0` [D 81].
fn cargo_status(root: &Path, args: &[String], env: &[(&str, &str)]) -> Result<ExitStatus> {
    let mut cmd = Command::new("cargo");
    cmd.args(args).current_dir(root).env("CARGO_INCREMENTAL", "0");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.status().with_context(|| format!("running cargo {}", args.join(" ")))
}

/// Whether `cargo <sub>` is installed (`cargo-<sub> --version`).
pub fn has_cargo_subcommand(sub: &str) -> bool {
    Command::new(format!("cargo-{sub}")).arg("--version").output().is_ok_and(|o| o.status.success())
        || Command::new("cargo").args([sub, "--version"]).output().is_ok_and(|o| o.status.success())
}

/// Whether a program runs (`<program> --version`).
pub fn has_program(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok_and(|o| o.status.success())
}

/// `cargo xtask schema --check`, run as a child of this very binary so W2's flags stay W2's. A stub that is not
/// implemented yet is a skip, not a failure.
fn schema_drift(root: &Path) -> Outcome {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return Outcome::Failed(format!("cannot find the xtask binary: {e}")),
    };
    let out = match Command::new(exe).args(["schema", "--check"]).current_dir(root).output() {
        Ok(o) => o,
        Err(e) => return Outcome::Failed(format!("running xtask schema --check: {e}")),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    print!("{stdout}");
    eprint!("{stderr}");
    if out.status.success() {
        Outcome::Ok(String::new())
    } else if stderr.contains("is not implemented yet") {
        Outcome::Skipped("W2's `cargo xtask schema` is not implemented yet".into())
    } else {
        Outcome::Failed("generated files are stale or the check failed: run `cargo xtask schema`".into())
    }
}

/// `cargo xtask test`: disk-guard, the due sweep, then `cargo nextest run ARGS` (or `cargo test ARGS`).
pub fn run_test(args: TestArgs) -> Result<()> {
    preflight()?;
    let root = workstreams::repo_root();
    let (sub, extra): (Vec<String>, Option<&str>) = if has_cargo_subcommand("nextest") {
        (vec!["nextest".into(), "run".into()], None)
    } else {
        (vec!["test".into()], Some("cargo-nextest is not installed: running cargo test with the same arguments"))
    };
    if let Some(note) = extra {
        println!("{note}");
    }
    let mut a = sub;
    a.extend(args.rest);
    let status = cargo_status(&root, &a, &[])?;
    if !status.success() {
        bail!("cargo {} failed with {status}", a.join(" "));
    }
    Ok(())
}

/// `cargo xtask build`: disk-guard, the due sweep, then `cargo build ARGS`.
pub fn run_build(args: BuildArgs) -> Result<()> {
    preflight()?;
    let root = workstreams::repo_root();
    let mut a = vec!["build".to_string()];
    a.extend(args.rest);
    let status = cargo_status(&root, &a, &[])?;
    if !status.success() {
        bail!("cargo {} failed with {status}", a.join(" "));
    }
    Ok(())
}

fn git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_ok(root: &Path, args: &[&str]) -> bool {
    Command::new("git").arg("-C").arg(root).args(args).output().is_ok_and(|o| o.status.success())
}

/// Workspace packages with a file changed against `base` (committed, uncommitted or untracked).
fn changed_packages(root: &Path, base: &str) -> Result<BTreeSet<String>> {
    let mut files = BTreeSet::new();
    if let Some(mb) = git_stdout(root, &["merge-base", base, "HEAD"]) {
        let range = format!("{}..HEAD", mb.trim());
        files.extend(git_stdout(root, &["diff", "--name-only", &range]).unwrap_or_default().lines().map(str::to_owned));
    }
    files.extend(git_stdout(root, &["diff", "--name-only", "HEAD"]).unwrap_or_default().lines().map(str::to_owned));
    files.extend(
        git_stdout(root, &["ls-files", "--others", "--exclude-standard"])
            .unwrap_or_default()
            .lines()
            .map(str::to_owned),
    );
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .context("cargo metadata")?;
    let mut dirs: Vec<(PathBuf, String)> = metadata
        .workspace_packages()
        .iter()
        .filter_map(|p| {
            let dir = p.manifest_path.parent()?.as_std_path();
            let rel = dir.strip_prefix(root).ok()?.to_path_buf();
            Some((rel, p.name.to_string()))
        })
        .collect();
    // The longest directory wins (a package nested in another package's directory).
    dirs.sort_by_key(|(d, _)| std::cmp::Reverse(d.components().count()));
    Ok(files
        .iter()
        .filter_map(|f| dirs.iter().find(|(d, _)| !d.as_os_str().is_empty() && Path::new(f).starts_with(d)))
        .map(|(_, name)| name.clone())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(steps: &[Step]) -> Vec<&'static str> {
        steps.iter().map(|s| s.name).collect()
    }

    #[test]
    fn the_pr_profile_runs_the_definition_of_done() {
        assert_eq!(
            names(&plan(Profile::Pr)),
            ["disk-guard", "fmt", "clippy", "test", "deny", "guard", "check-layers", "check-paths", "schema"]
        );
        let deny = plan(Profile::Pr).into_iter().find(|s| s.name == "deny").expect("deny");
        assert_eq!(deny.action, Action::Deny(strings(&["check", "licenses", "bans", "sources"])));
        let clippy = plan(Profile::Pr).into_iter().find(|s| s.name == "clippy").expect("clippy");
        assert!(describe(&clippy.action).ends_with("-- -D warnings"));
    }

    #[test]
    fn the_full_profile_adds_features_docs_and_advisories() {
        let full = names(&plan(Profile::Full));
        for extra in ["clippy-all-features", "doctest", "doc"] {
            assert!(full.contains(&extra), "{full:?} lacks {extra}");
        }
        let deny = plan(Profile::Full).into_iter().find(|s| s.name == "deny").expect("deny");
        assert_eq!(deny.action, Action::Deny(strings(&["check"])));
        let doc = plan(Profile::Full).into_iter().find(|s| s.name == "doc").expect("doc");
        assert_eq!(doc.env, vec![("RUSTDOCFLAGS", "-D warnings")]);
    }

    #[test]
    fn the_windows_profile_never_relinks_a_running_xtask() {
        for s in plan(Profile::Windows) {
            let text = describe(&s.action);
            if text.starts_with("cargo") && s.name != "disk-guard" {
                assert!(text.contains("--exclude xtask"), "{text}");
            }
        }
    }

    #[test]
    fn every_profile_starts_with_the_disk_guard() {
        for p in [Profile::Pr, Profile::Full, Profile::Windows] {
            assert_eq!(plan(p)[0].action, Action::DiskGuard);
        }
        assert_eq!(quick_plan(&BTreeSet::new())[0].action, Action::DiskGuard);
    }

    #[test]
    fn the_quick_plan_targets_changed_packages() {
        assert_eq!(names(&quick_plan(&BTreeSet::new())), ["disk-guard", "fmt"]);
        let changed: BTreeSet<String> = ["russet-testkit".to_string(), "xtask".to_string()].into();
        let q = quick_plan(&changed);
        assert_eq!(names(&q), ["disk-guard", "fmt", "clippy", "test"]);
        let clippy = describe(&q[2].action);
        assert!(clippy.contains("-p russet-testkit") && clippy.contains("-p xtask"), "{clippy}");
    }

    #[test]
    fn changed_files_map_to_their_packages() {
        let root = workstreams::repo_root();
        // Whatever the branch state, every reported package is a workspace member.
        let changed = changed_packages(&root, "main").expect("changed packages");
        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(root.join("Cargo.toml"))
            .no_deps()
            .exec()
            .expect("metadata");
        let members: BTreeSet<String> = metadata.workspace_packages().iter().map(|p| p.name.to_string()).collect();
        assert!(changed.is_subset(&members), "{changed:?}");
    }
}
