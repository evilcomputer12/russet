// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask guard [CHECK…]`: the repository guards (QA-02; ARCHITECTURE §12.2, §19; PLAN W8).
//!
//! | Check | What it fails on |
//! |---|---|
//! | `apple-files` (alias `check-no-apple`) | Apple-format magic, Apple file extensions and Apple copyright headers ([`crate::no_apple`]), except generated fixtures that the provenance allowlist admits ([`provenance_allows`], [D 88]) |
//! | `secrets` | the built-in scan (PEM and PGP private keys, AWS, GitHub, Slack and Google keys, JWTs, real-looking UDIDs, the owner's local denylist of private identifiers) and gitleaks over the branch's history when installed; reviewed synthetic test values ([`SYNTHETIC_ALLOWLIST`]) pass |
//! | `reuse` | `reuse lint` (REUSE 3.3: SPDX headers everywhere) |
//! | `typos` | `typos` with the reviewed allowlist in `typos.toml` |
//! | `public-terms` | the private track's identifying terms, compared as SHA-256 digests of lowercased tokens and token pairs, outside the allowlist ([D 91]) |
//! | `zizmor` | GitHub Actions findings of medium severity or above (offline audits; `--zizmor-online` adds the online ones) |
//! | `commits` | commit subjects of the branch that are not Conventional Commits |
//! | `dco` | commits of the branch without a `Signed-off-by:` trailer (not in `all` during the private phase; CONTRIBUTING.md) |
//!
//! `all` (the default) runs every check but `dco`. `--artifacts PATH` makes `apple-files` and `secrets` scan that
//! file or directory instead of the git tree (the `upload-safe` action, the CI cache), following symlinks the way
//! `actions/upload-artifact` does and refusing globs it would expand; `--upload-allowlist` with `--workflow` and
//! `--job` then admits the Mach-O outputs that `.github/upload-safe.toml` names for that job. An external tool
//! that is missing skips its check with a note, unless `--require-tools` (CI). Findings never print a secret's
//! value; `--suggest-allowlist` (never on CI) prints reviewed-allowlist entries for them. Owner: W8.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use globset::{Glob, GlobSetBuilder};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::no_apple::{self, Finding};

/// Flags of `cargo xtask guard`.
#[derive(Debug, Default, clap::Args)]
pub struct Args {
    /// Guards to run: apple-files, secrets, reuse, typos, public-terms, zizmor, commits, dco, all (default: all)
    #[arg(value_name = "CHECK")]
    pub checks: Vec<String>,
    /// Scan this file or directory (an upload, the CI cache) instead of the git tree, for apple-files and secrets
    #[arg(long = "artifacts", value_name = "PATH")]
    pub artifacts: Vec<PathBuf>,
    /// What a missing --artifacts path means (the upload-safe action passes its if-no-files-found)
    #[arg(long, value_enum, default_value_t = IfMissing::Error)]
    pub if_missing: IfMissing,
    /// The upload allowlist (.github/upload-safe.toml): Mach-O outputs a workflow job may upload
    #[arg(long, value_name = "FILE", requires_all = ["workflow", "job"])]
    pub upload_allowlist: Option<PathBuf>,
    /// The workflow file name of the uploading job (apple-release.yml)
    #[arg(long, value_name = "FILE")]
    pub workflow: Option<String>,
    /// The id of the uploading job
    #[arg(long, value_name = "ID")]
    pub job: Option<String>,
    /// Fail instead of skipping when reuse, typos, gitleaks or zizmor is missing (CI)
    #[arg(long)]
    pub require_tools: bool,
    /// Base of the commit range for commits and dco (default: the merge base with main, else origin/main)
    #[arg(long, value_name = "REF")]
    pub base: Option<String>,
    /// More public-terms digests: a file with one lowercase hex SHA-256 per line
    #[arg(long, value_name = "FILE")]
    pub term_digests: Option<PathBuf>,
    /// Read terms from stdin, one per line, print their digests and exit (for the compiled-in list)
    #[arg(long)]
    pub hash_terms: bool,
    /// The owner's local denylist of private identifiers (default: ~/.config/russet/private-identifiers)
    #[arg(long, value_name = "FILE")]
    pub private_identifiers: Option<PathBuf>,
    /// The reviewed allowlist of synthetic test values (default: .github/synthetic-secrets.toml)
    #[arg(long, value_name = "FILE")]
    pub synthetic_allowlist: Option<PathBuf>,
    /// Print allowlist entries (with the values' SHA-256) for the secrets findings; local only, never on CI
    #[arg(long)]
    pub suggest_allowlist: bool,
    /// Run zizmor's online audits too (impostor commits, known-vulnerable actions); needs GH_TOKEN
    #[arg(long)]
    pub zizmor_online: bool,
}

/// What a missing `--artifacts` path means. `actions/upload-artifact` uploads nothing for it either, so `warn` and
/// `ignore` are safe once the path is known to be a plain path ([`plain_path`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum IfMissing {
    /// Fail (the default outside the upload-safe action).
    #[default]
    Error,
    /// Print a warning and scan nothing (upload-artifact's default `if-no-files-found: warn`).
    Warn,
    /// Scan nothing, silently.
    Ignore,
}

/// One guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    AppleFiles,
    Secrets,
    Reuse,
    Typos,
    PublicTerms,
    Zizmor,
    Commits,
    Dco,
    All,
}

impl Check {
    /// What `all` runs: everything but the DCO check (CONTRIBUTING.md: enforced on pull requests once public).
    pub const ALL: &[Check] = &[
        Check::AppleFiles,
        Check::Secrets,
        Check::Reuse,
        Check::Typos,
        Check::PublicTerms,
        Check::Zizmor,
        Check::Commits,
    ];

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "apple-files" | "check-no-apple" => Self::AppleFiles,
            "secrets" => Self::Secrets,
            "reuse" => Self::Reuse,
            "typos" => Self::Typos,
            "public-terms" => Self::PublicTerms,
            "zizmor" => Self::Zizmor,
            "commits" => Self::Commits,
            "dco" => Self::Dco,
            "all" => Self::All,
            other => bail!(
                "unknown guard {other:?}: apple-files, secrets, reuse, typos, public-terms, zizmor, commits, dco, all"
            ),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::AppleFiles => "apple-files",
            Self::Secrets => "secrets",
            Self::Reuse => "reuse",
            Self::Typos => "typos",
            Self::PublicTerms => "public-terms",
            Self::Zizmor => "zizmor",
            Self::Commits => "commits",
            Self::Dco => "dco",
            Self::All => "all",
        }
    }
}

/// Everything a guard run needs besides the repository root.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub artifacts: Vec<PathBuf>,
    pub if_missing: IfMissing,
    pub upload: Option<UploadAllow>,
    pub require_tools: bool,
    pub base: Option<String>,
    /// Digests added to [`PUBLIC_TERM_DIGESTS`].
    pub extra_digests: Vec<[u8; 32]>,
    /// The owner's denylist file; `None` reads the default location if it exists.
    pub private_identifiers: Option<PathBuf>,
    /// Never read the default denylist (tests).
    pub no_default_denylist: bool,
    /// The synthetic allowlist; `None` reads [`SYNTHETIC_ALLOWLIST`] under the root if it exists.
    pub synthetic_allowlist: Option<PathBuf>,
    /// Collect allowlist entries for the secrets findings.
    pub suggest_allowlist: bool,
    pub zizmor_online: bool,
    /// Never run external tools (tests of the built-in scans).
    pub builtin_only: bool,
}

/// The result of a guard run.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    /// Checks with findings or errors.
    pub failed: Vec<String>,
    /// External tools that were missing.
    pub missing_tools: Vec<String>,
    /// Checks that were skipped, with the reason.
    pub skipped: Vec<(String, String)>,
    /// Every finding, `check: text`.
    pub findings: Vec<String>,
    pub elapsed: Duration,
}

impl Summary {
    /// A one-line note for `cargo xtask ci`'s summary.
    pub fn note(&self) -> String {
        let mut s = format!("{:.1}s", self.elapsed.as_secs_f64());
        if !self.skipped.is_empty() {
            let names: Vec<&str> = self.skipped.iter().map(|(n, _)| n.as_str()).collect();
            s.push_str(&format!("; skipped: {}", names.join(", ")));
        }
        s
    }
}

pub fn run(args: Args) -> Result<()> {
    if args.hash_terms {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).context("reading terms from stdin")?;
        for digest in hash_terms(&text)? {
            println!("{}", hex(&digest));
        }
        return Ok(());
    }
    if args.suggest_allowlist && crate::ci::on_ci() {
        // The digest of a real identifier is brute-forceable; it never goes into a CI log.
        bail!("--suggest-allowlist runs only locally (CI is set): review the findings on your machine");
    }
    let root = crate::workstreams::repo_root();
    let mut checks = Vec::new();
    for c in &args.checks {
        checks.push(Check::parse(c)?);
    }
    if checks.is_empty() {
        checks.push(if args.artifacts.is_empty() { Check::All } else { Check::AppleFiles });
        if !args.artifacts.is_empty() {
            checks.push(Check::Secrets);
        }
    }
    let upload = match &args.upload_allowlist {
        Some(file) => Some(UploadAllow::load(
            file,
            args.workflow.as_deref().unwrap_or_default(),
            args.job.as_deref().unwrap_or_default(),
        )?),
        None => None,
    };
    let extra_digests = match &args.term_digests {
        Some(f) => read_digests(f)?,
        None => Vec::new(),
    };
    let opts = Options {
        artifacts: args.artifacts,
        if_missing: args.if_missing,
        upload,
        require_tools: args.require_tools,
        base: args.base,
        extra_digests,
        private_identifiers: args.private_identifiers,
        no_default_denylist: false,
        synthetic_allowlist: args.synthetic_allowlist,
        suggest_allowlist: args.suggest_allowlist,
        zizmor_online: args.zizmor_online,
        builtin_only: false,
    };
    let summary = run_checks(&root, &checks, &opts)?;
    if !summary.missing_tools.is_empty() {
        println!(
            "guard: missing optional tools: {} (install: {})",
            summary.missing_tools.join(", "),
            crate::ci::PACMAN_LINE
        );
    }
    if summary.failed.is_empty() {
        println!("guard: ok ({})", summary.note());
        Ok(())
    } else {
        bail!("guard: {} check(s) failed: {}", summary.failed.len(), summary.failed.join(", "))
    }
}

/// Runs `checks` over the repository at `root` (or over `opts.artifacts`), printing findings as it goes.
pub fn run_checks(root: &Path, checks: &[Check], opts: &Options) -> Result<Summary> {
    let start = Instant::now();
    let mut list: Vec<Check> = Vec::new();
    for c in checks {
        if *c == Check::All {
            list.extend_from_slice(Check::ALL);
        } else {
            list.push(*c);
        }
    }
    list.sort();
    list.dedup();
    let files = if opts.artifacts.is_empty() { Some(tree_files(root)?) } else { None };
    let mut summary = Summary::default();
    for check in list {
        let t = Instant::now();
        let result = match check {
            Check::AppleFiles => apple_files(root, files.as_deref(), opts),
            Check::Secrets => secrets(root, files.as_deref(), opts),
            Check::Reuse => reuse(root, files.as_deref(), opts),
            Check::Typos => external_tree(opts, "typos", root, |cmd| {
                cmd.arg("--format").arg("brief");
            }),
            Check::PublicTerms => match &files {
                Some(f) => public_terms(root, f, opts),
                None => Ok(CheckResult::skip("artifact scans do not check public terms")),
            },
            Check::Zizmor => zizmor(root, opts),
            Check::Commits => commit_check(root, opts, CommitRule::Conventional),
            Check::Dco => commit_check(root, opts, CommitRule::SignedOff),
            Check::All => unreachable!("expanded above"),
        };
        let result = result.unwrap_or_else(|e| CheckResult::error(format!("{e:#}")));
        for f in &result.findings {
            println!("{}: {f}", check.name());
            summary.findings.push(format!("{}: {f}", check.name()));
        }
        if opts.suggest_allowlist && !result.suggestions.is_empty() {
            println!(
                "# Entries for {SYNTHETIC_ALLOWLIST}: add one only after checking that its value is synthetic (a test \
                 value, never a real key, token or device identifier), and say why in `reason`."
            );
            for s in &result.suggestions {
                println!("{s}");
            }
        }
        if let Some(tool) = &result.missing_tool {
            summary.missing_tools.push(tool.clone());
        }
        let took = t.elapsed().as_secs_f64();
        match (&result.skipped, result.findings.is_empty() && result.error.is_none()) {
            (Some(why), _) => {
                println!("guard {}: skipped: {why}", check.name());
                if opts.require_tools && result.missing_tool.is_some() {
                    summary.failed.push(check.name().to_string());
                }
                summary.skipped.push((check.name().to_string(), why.clone()));
            }
            (None, true) => println!("guard {}: ok{} ({took:.1}s)", check.name(), result.note_suffix()),
            (None, false) => {
                if let Some(e) = &result.error {
                    println!("guard {}: error: {e}", check.name());
                }
                println!("guard {}: FAILED ({} finding(s), {took:.1}s)", check.name(), result.findings.len());
                summary.failed.push(check.name().to_string());
            }
        }
    }
    summary.elapsed = start.elapsed();
    Ok(summary)
}

/// What one check found.
#[derive(Debug, Clone, Default)]
struct CheckResult {
    findings: Vec<String>,
    /// Allowlist entries for the findings (`--suggest-allowlist`).
    suggestions: Vec<String>,
    skipped: Option<String>,
    missing_tool: Option<String>,
    error: Option<String>,
    note: Option<String>,
}

impl CheckResult {
    fn skip(why: impl Into<String>) -> Self {
        Self { skipped: Some(why.into()), ..Self::default() }
    }

    fn missing(tool: &str) -> Self {
        Self { skipped: Some(format!("{tool} is not installed")), missing_tool: Some(tool.into()), ..Self::default() }
    }

    fn error(e: String) -> Self {
        Self { error: Some(e), ..Self::default() }
    }

    fn note_suffix(&self) -> String {
        self.note.as_ref().map(|n| format!(": {n}")).unwrap_or_default()
    }
}

// ---- files -------------------------------------------------------------------------------------------------

/// Tracked files plus untracked files that are not ignored, repository-relative with `/`; deleted files are
/// skipped.
pub fn tree_files(root: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output()
        .context("running git ls-files")?;
    if !out.status.success() {
        bail!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let mut files: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .filter(|rel| root.join(rel).is_file())
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Every regular file below `path` (or `path` itself), following symlinks as `actions/upload-artifact` does
/// (its globber follows them by default), so a link to an Apple file is scanned where the upload would read it.
/// `stack` holds the canonical directories being walked, which stops symlink loops; a directory reached through
/// two different paths is walked twice, because the upload carries it twice. A dangling link becomes a note.
fn walk_artifact(path: &Path, out: &mut Vec<PathBuf>, stack: &mut Vec<PathBuf>, notes: &mut Vec<String>) -> Result<()> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if std::fs::symlink_metadata(path).is_ok() => {
            notes.push(format!("{}: a dangling symlink ({e}); nothing to scan", path.display()));
            return Ok(());
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if meta.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    if !meta.is_dir() {
        // Sockets and FIFOs carry no file content to upload.
        return Ok(());
    }
    let canonical = std::fs::canonicalize(path).with_context(|| format!("resolving {}", path.display()))?;
    if stack.contains(&canonical) {
        notes.push(format!("{}: a symlink loop; not followed again", path.display()));
        return Ok(());
    }
    stack.push(canonical);
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .and_then(|rd| rd.collect::<std::io::Result<Vec<_>>>())
        .with_context(|| format!("listing {}", path.display()))?;
    entries.sort_by_key(|e| e.path());
    for e in entries {
        walk_artifact(&e.path(), out, stack, notes)?;
    }
    stack.pop();
    Ok(())
}

/// Refuses an `--artifacts` path that `actions/upload-artifact` would read differently from the guard: glob
/// characters (`* ? [ ] { }`), a `!` exclusion, a leading `~`, or several lines. The guard scans exactly one
/// literal file or directory, so the upload must name exactly that.
pub fn plain_path(path: &Path) -> Result<()> {
    let s = path.to_string_lossy();
    if s.trim().is_empty() {
        bail!("an empty --artifacts path");
    }
    if s.contains(['\n', '\r']) {
        bail!("--artifacts {s:?} has several lines, which actions/upload-artifact would expand: one path per flag");
    }
    if s.contains(['*', '?', '[', ']', '{', '}']) {
        bail!(
            "--artifacts {s:?} has glob characters, which actions/upload-artifact would expand: pass one plain file \
             or directory"
        );
    }
    if s.starts_with('!') || s.starts_with('~') {
        bail!("--artifacts {s:?}: a `!` exclusion or a `~` path is expanded by actions/upload-artifact, not here");
    }
    Ok(())
}

/// One file to scan.
struct ScanItem {
    /// How findings name it: repository-relative, or the path as found under an artifact directory.
    shown: String,
    abs: PathBuf,
    /// Relative to the repository or to the artifact directory (what the upload allowlist matches).
    rel: String,
}

/// Every file to scan, plus notes (artifact paths that do not exist under `--if-missing warn`, dangling links).
fn scan_set(root: &Path, files: Option<&[String]>, opts: &Options) -> Result<(Vec<ScanItem>, Vec<String>)> {
    let mut notes = Vec::new();
    let mut out = Vec::new();
    match files {
        Some(files) => {
            out.extend(files.iter().map(|rel| ScanItem { shown: rel.clone(), abs: root.join(rel), rel: rel.clone() }))
        }
        None => {
            for dir in &opts.artifacts {
                plain_path(dir)?;
                // Neither a file nor a (possibly dangling) link: the upload finds nothing there either.
                if std::fs::symlink_metadata(dir).is_err() {
                    match opts.if_missing {
                        IfMissing::Error => bail!("{} does not exist (--if-missing error)", dir.display()),
                        IfMissing::Warn => {
                            notes.push(format!("warning: {} does not exist; nothing to scan", dir.display()))
                        }
                        IfMissing::Ignore => {}
                    }
                    continue;
                }
                let mut found = Vec::new();
                walk_artifact(dir, &mut found, &mut Vec::new(), &mut notes)?;
                let single_file = dir.is_file();
                for p in found {
                    let rel = if single_file {
                        p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    } else {
                        slash(p.strip_prefix(dir).unwrap_or(&p))
                    };
                    out.push(ScanItem { shown: p.display().to_string(), abs: p.clone(), rel });
                }
            }
        }
    }
    Ok((out, notes))
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Text content for the text scans: `None` for binary files (a NUL byte in the first 8 KiB) and files over
/// 8 MiB.
fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 8 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

// ---- apple-files and the provenance allowlist ----------------------------------------------------------

/// The only roots whose `PROVENANCE.toml` the guard reads (ARCHITECTURE §19 [D 88]). A new root is a lead PR.
pub const PROVENANCE_ROOTS: &[&str] = &["tests/fixtures/xip/", "fuzz/russet-xar/corpus/"];

/// Whether a provenance allowlist admits an Apple-format file (ARCHITECTURE §19 [D 88]): only under the roots
/// compiled in here (`tests/fixtures/xip/`, `fuzz/russet-xar/corpus/`), only when that root's
/// `PROVENANCE.toml` lists the file with a matching SHA-256 and a committed generator script.
pub fn provenance_allows(root: &Path, finding: &Finding) -> bool {
    matches!(provenance_verdict(root, &finding.path), Verdict::Allowed)
}

/// How the provenance allowlist treats a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Listed with a matching hash and a committed generator.
    Allowed,
    /// Not under a provenance root.
    NotUnderRoot,
    /// Under a root, but refused, with the reason.
    Rejected(String),
}

/// `PROVENANCE.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvenanceFile {
    schema: u32,
    #[serde(default, rename = "file")]
    files: Vec<ProvenanceEntry>,
}

/// One generated file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvenanceEntry {
    /// Relative to the provenance root.
    path: String,
    /// Lowercase hex SHA-256 of the committed bytes.
    sha256: String,
    /// The repository-relative, committed script that produced the file from open tools.
    generator: String,
    #[serde(default)]
    #[allow(dead_code)] // documentation for reviewers; not checked
    description: Option<String>,
}

/// The provenance verdict for a repository-relative path.
pub fn provenance_verdict(root: &Path, rel: &str) -> Verdict {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_start_matches("./");
    let Some(prov_root) = PROVENANCE_ROOTS.iter().find(|r| rel.starts_with(**r)) else {
        return Verdict::NotUnderRoot;
    };
    let manifest_path = root.join(prov_root).join("PROVENANCE.toml");
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(t) => t,
        Err(_) => return Verdict::Rejected(format!("no {prov_root}PROVENANCE.toml")),
    };
    let manifest: ProvenanceFile = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Verdict::Rejected(format!("{prov_root}PROVENANCE.toml is invalid: {e}")),
    };
    if manifest.schema != 1 {
        return Verdict::Rejected(format!("{prov_root}PROVENANCE.toml has unknown schema {}", manifest.schema));
    }
    let inner = &rel[prov_root.len()..];
    let Some(entry) = manifest.files.iter().find(|e| e.path == inner) else {
        return Verdict::Rejected(format!("not listed in {prov_root}PROVENANCE.toml"));
    };
    if entry.path.starts_with('/') || entry.path.split('/').any(|c| c == ".." || c.is_empty()) {
        return Verdict::Rejected(format!("bad path {:?} in {prov_root}PROVENANCE.toml", entry.path));
    }
    let actual = match std::fs::read(root.join(rel)) {
        Ok(bytes) => hex(&Sha256::digest(&bytes)),
        Err(e) => return Verdict::Rejected(format!("cannot read it: {e}")),
    };
    if !actual.eq_ignore_ascii_case(entry.sha256.trim()) {
        return Verdict::Rejected(format!(
            "its SHA-256 {}… differs from the {}… listed in {prov_root}PROVENANCE.toml",
            &actual[..12],
            entry.sha256.get(..12).unwrap_or(&entry.sha256)
        ));
    }
    let generator = entry.generator.trim_start_matches("./");
    if generator.starts_with('/') || generator.split('/').any(|c| c == "..") || !root.join(generator).is_file() {
        return Verdict::Rejected(format!("its generator {:?} does not exist in the repository", entry.generator));
    }
    let tracked = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--error-unmatch", "--", generator])
        .output()
        .is_ok_and(|o| o.status.success());
    if !tracked {
        return Verdict::Rejected(format!("its generator {generator} is not committed"));
    }
    Verdict::Allowed
}

fn apple_files(root: &Path, files: Option<&[String]>, opts: &Options) -> Result<CheckResult> {
    let (set, notes) = scan_set(root, files, opts)?;
    let mut r = CheckResult::default();
    for ScanItem { shown, abs, rel } in &set {
        let reasons = no_apple::scan_file(abs).with_context(|| format!("reading {shown}"))?;
        if reasons.is_empty() {
            continue;
        }
        if files.is_some() {
            match provenance_verdict(root, shown) {
                Verdict::Allowed => continue,
                Verdict::NotUnderRoot => r.findings.push(format!("{shown}: {}", reasons.join(", "))),
                Verdict::Rejected(why) => {
                    r.findings.push(format!("{shown}: {} (provenance: {why})", reasons.join(", ")));
                }
            }
        } else if opts.upload.as_ref().is_some_and(|u| u.admits(rel, &reasons)) {
            continue;
        } else {
            r.findings.push(format!("{shown}: {}", reasons.join(", ")));
        }
    }
    r.note = Some(format!("{} files scanned{}", set.len(), notes.iter().map(|n| format!("; {n}")).collect::<String>()));
    Ok(r)
}

/// The Mach-O outputs one workflow job may upload (`.github/upload-safe.toml`).
#[derive(Debug, Clone)]
pub struct UploadAllow {
    globs: globset::GlobSet,
    count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadFile {
    schema: u32,
    #[serde(default, rename = "allow")]
    allow: Vec<UploadEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadEntry {
    workflow: String,
    job: String,
    /// Globs relative to the uploaded directory.
    paths: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)] // for reviewers
    reason: Option<String>,
}

impl UploadAllow {
    /// The entries of `file` for this workflow file name and job id.
    pub fn load(file: &Path, workflow: &str, job: &str) -> Result<Self> {
        let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        Self::parse(&text, workflow, job).with_context(|| format!("parsing {}", file.display()))
    }

    pub fn parse(text: &str, workflow: &str, job: &str) -> Result<Self> {
        let parsed: UploadFile = toml::from_str(text)?;
        anyhow::ensure!(parsed.schema == 1, "unknown schema {}", parsed.schema);
        let mut b = GlobSetBuilder::new();
        let mut count = 0;
        for e in parsed.allow.iter().filter(|e| e.workflow == workflow && e.job == job) {
            for p in &e.paths {
                b.add(Glob::new(p).with_context(|| format!("bad glob {p:?}"))?);
                count += 1;
            }
        }
        Ok(Self { globs: b.build()?, count })
    }

    /// Only Mach-O findings, and only at the listed paths: every other Apple format stays blocked.
    pub fn admits(&self, rel: &str, reasons: &[&str]) -> bool {
        self.count > 0 && reasons.iter().all(|r| r.contains("Mach-O")) && self.globs.is_match(rel)
    }
}

// ---- secrets -----------------------------------------------------------------------------------------------

/// A secret-shaped match: the rule, its 1-based position and the SHA-256 of the matched value. Only the reviewed
/// synthetic allowlist reads the digest ([`SyntheticAllowlist`]); findings never print it, and the value itself is
/// never kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub rule: &'static str,
    pub line: usize,
    pub col: usize,
    /// `None` for the owner's private identifiers, which no allowlist admits.
    pub digest: Option<[u8; 32]>,
}

impl Hit {
    fn new(rule: &'static str, line: usize, col: usize, value: &[u8]) -> Self {
        Self { rule, line, col, digest: Some(value_digest(rule, value)) }
    }
}

/// The SHA-256 the synthetic allowlist compares: of the value as found, upper-cased for UDIDs (hex digits are
/// case-insensitive).
pub fn value_digest(rule: &str, value: &[u8]) -> [u8; 32] {
    if rule == "udid" { Sha256::digest(value.to_ascii_uppercase()).into() } else { Sha256::digest(value).into() }
}

/// Lines carrying one of these markers are not reported (gitleaks' own marker is honoured too), except for the
/// owner's private identifiers, which no marker hides.
const ALLOW_MARKERS: &[&str] = &["gitleaks:allow", "russet-guard:allow"];

/// The reviewed allowlist of synthetic test values (owner review through `.github/CODEOWNERS`).
pub const SYNTHETIC_ALLOWLIST: &str = ".github/synthetic-secrets.toml";

/// Synthetic test values shaped like secrets or device identifiers (a test JWT, a UDID with a hand-typed ECID),
/// admitted by the SHA-256 of the value ([`value_digest`]; gitleaks findings by the digest of the secret it
/// reports). Value digests, not file positions, so an entry survives rebases and covers the history gitleaks
/// reads; a value that is not listed fails wherever it appears.
#[derive(Debug, Clone, Default)]
pub struct SyntheticAllowlist {
    digests: HashSet<[u8; 32]>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticFile {
    schema: u32,
    #[serde(default)]
    synthetic: Vec<SyntheticEntry>,
}

/// One reviewed value.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticEntry {
    /// The rule that found it (`udid`, `jwt`, or a gitleaks rule id such as `generic-api-key`); documentation.
    rule: String,
    /// SHA-256 of the value ([`value_digest`]).
    sha256: String,
    /// The workstream (or `lead`) whose test uses the value.
    owner: String,
    /// Where it is and why it is synthetic.
    reason: String,
}

impl SyntheticAllowlist {
    pub fn parse(text: &str) -> Result<Self> {
        let file: SyntheticFile = toml::from_str(text)?;
        anyhow::ensure!(file.schema == 1, "unknown schema {}", file.schema);
        let mut digests = HashSet::new();
        for e in &file.synthetic {
            anyhow::ensure!(!e.rule.trim().is_empty(), "an entry without a rule");
            anyhow::ensure!(!e.owner.trim().is_empty(), "the entry {} has no owner", e.sha256);
            anyhow::ensure!(e.reason.trim().len() >= 10, "the entry {} needs a reason", e.sha256);
            digests.insert(parse_digest(e.sha256.trim()).context("sha256")?);
        }
        Ok(Self { digests })
    }

    /// `--synthetic-allowlist FILE`, else [`SYNTHETIC_ALLOWLIST`] under `root` if it exists.
    fn load(root: &Path, opts: &Options) -> Result<Self> {
        let (path, required) = match &opts.synthetic_allowlist {
            Some(p) => (p.clone(), true),
            None => (root.join(SYNTHETIC_ALLOWLIST), false),
        };
        if !required && !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn admits(&self, digest: &[u8; 32]) -> bool {
        self.digests.contains(digest)
    }
}

/// An allowlist entry for `--suggest-allowlist`.
fn suggestion(rule: &str, digest: &[u8; 32], place: &str) -> String {
    format!(
        "[[synthetic]]\nrule = \"{rule}\"\nsha256 = \"{}\"\nowner = \"W??\"\nreason = \"{place}: why the value is synthetic\"\n",
        hex(digest)
    )
}

fn secrets(root: &Path, files: Option<&[String]>, opts: &Options) -> Result<CheckResult> {
    let (set, notes) = scan_set(root, files, opts)?;
    let denylist = load_denylist(opts)?;
    let synthetic = SyntheticAllowlist::load(root, opts)?;
    let mut r = CheckResult::default();
    let mut admitted = 0;
    let mut suggested: HashSet<[u8; 32]> = HashSet::new();
    for ScanItem { shown, abs, .. } in &set {
        let Some(text) = read_text(abs) else { continue };
        for hit in scan_secrets(&text, &denylist) {
            if hit.digest.is_some_and(|d| synthetic.admits(&d)) {
                admitted += 1;
                continue;
            }
            r.findings.push(format!("{shown}:{}:{}: {} (value not shown)", hit.line, hit.col, hit.rule));
            if let Some(d) = hit.digest
                && opts.suggest_allowlist
                && suggested.insert(d)
            {
                r.suggestions.push(suggestion(hit.rule, &d, &format!("{shown}:{}", hit.line)));
            }
        }
    }
    let mut note = format!("built-in scan of {} files", set.len());
    if !denylist.is_empty() {
        note.push_str(&format!(", {} private identifier(s)", denylist.len()));
    }
    // gitleaks adds its rule set and the branch's history when installed.
    if !opts.builtin_only {
        if crate::ci::has_program("gitleaks") {
            let g = gitleaks(root, files.is_some(), opts, &synthetic)?;
            admitted += g.admitted;
            r.findings.extend(g.findings);
            for (rule, d, place) in g.unlisted {
                if opts.suggest_allowlist && suggested.insert(d) {
                    r.suggestions.push(suggestion(&rule, &d, &place));
                }
            }
            note.push_str(&format!("; gitleaks: {}", g.summary));
        } else {
            note.push_str("; gitleaks not installed");
            r.missing_tool = Some("gitleaks".into());
            if opts.require_tools {
                r.error = Some("gitleaks is not installed (--require-tools)".into());
            }
        }
    }
    if admitted > 0 {
        note.push_str(&format!("; {admitted} reviewed synthetic value(s) admitted ({SYNTHETIC_ALLOWLIST})"));
    }
    for n in notes {
        note.push_str(&format!("; {n}"));
    }
    r.note = Some(note);
    Ok(r)
}

/// What gitleaks found once the synthetic allowlist is applied.
#[derive(Debug, Default)]
struct GitleaksOutcome {
    findings: Vec<String>,
    /// `(rule, digest, place)` of every finding, for `--suggest-allowlist`.
    unlisted: Vec<(String, [u8; 32], String)>,
    admitted: usize,
    summary: String,
}

/// Applies the synthetic allowlist to a gitleaks JSON report (an array of findings with `RuleID`, `File`,
/// `StartLine`, `Commit` and `Secret`), adding what remains to `out` without the values. Returns how many findings
/// the report held.
fn filter_gitleaks_report(report: &str, synthetic: &SyntheticAllowlist, out: &mut GitleaksOutcome) -> Result<usize> {
    let items: Vec<serde_json::Value> = serde_json::from_str(report).context("parsing the gitleaks report")?;
    for item in &items {
        let field = |k: &str| item.get(k).and_then(serde_json::Value::as_str).unwrap_or_default();
        let digest: [u8; 32] = Sha256::digest(field("Secret").as_bytes()).into();
        if synthetic.admits(&digest) {
            out.admitted += 1;
            continue;
        }
        let line = item.get("StartLine").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let place = format!("{}:{line}", field("File"));
        let commit = field("Commit");
        let commit =
            if commit.is_empty() { String::new() } else { format!(", commit {}", commit.get(..10).unwrap_or(commit)) };
        out.findings.push(format!("{place}: {} (gitleaks{commit}; value not shown)", field("RuleID")));
        out.unlisted.push((field("RuleID").to_string(), digest, place));
    }
    Ok(items.len())
}

/// gitleaks' exit code for "leaks found", distinct from its exit code 1 for errors.
const GITLEAKS_LEAKS_EXIT: i32 = 3;

/// Runs gitleaks with a JSON report in a private temporary directory (it holds the matched values, so it is never
/// printed and is deleted at once), then applies the synthetic allowlist to the reported secrets. The git tree
/// is scanned through the checked-out branch's history (`--log-opts=HEAD`): a local repository shared by several
/// worktrees holds every card's branches, and gitleaks' default would read them all. A `.gitleaksignore` in the
/// repository root is honoured, as gitleaks does by itself.
fn gitleaks(root: &Path, tree: bool, opts: &Options, synthetic: &SyntheticAllowlist) -> Result<GitleaksOutcome> {
    let scratch = ScratchDir::new("russet-gitleaks")?;
    let report = scratch.0.join("report.json");
    let mut targets: Vec<Vec<std::ffi::OsString>> = Vec::new();
    if tree {
        targets.push(vec!["git".into(), "--log-opts=HEAD".into(), root.as_os_str().to_owned()]);
    } else {
        for dir in opts.artifacts.iter().filter(|d| d.exists()) {
            targets.push(vec!["dir".into(), "--follow-symlinks".into(), dir.as_os_str().to_owned()]);
        }
    }
    let mut out = GitleaksOutcome::default();
    let mut reported = 0;
    for target in &targets {
        let _ = std::fs::remove_file(&report);
        let output = Command::new("gitleaks")
            .args(&target[..1])
            .args(["--no-banner", "--no-color", "--log-level", "error", "--report-format", "json"])
            .arg("--report-path")
            .arg(&report)
            .args(["--exit-code", &GITLEAKS_LEAKS_EXIT.to_string()])
            .args(&target[1..])
            .current_dir(root)
            .output()
            .context("running gitleaks")?;
        match output.status.code() {
            Some(0) => {}
            Some(GITLEAKS_LEAKS_EXIT) => {
                let text = std::fs::read_to_string(&report).context("reading the gitleaks report")?;
                let _ = std::fs::remove_file(&report);
                reported += filter_gitleaks_report(&text, synthetic, &mut out)?;
            }
            _ => bail!("gitleaks failed with {}: {}", output.status, String::from_utf8_lossy(&output.stderr).trim()),
        }
    }
    out.summary = if reported == 0 {
        "no leaks".into()
    } else {
        format!("{reported} report(s), {} reviewed synthetic", out.admitted)
    };
    Ok(out)
}

/// The owner's local denylist (QA §3.5.3): one identifier per line, `#` comments. Never printed.
fn load_denylist(opts: &Options) -> Result<Vec<String>> {
    let path = match &opts.private_identifiers {
        Some(p) => Some(p.clone()),
        None if opts.no_default_denylist => None,
        None => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|c| c.join("russet/private-identifiers"))
            .filter(|p| p.is_file()),
    };
    let Some(path) = path else { return Ok(Vec::new()) };
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        // Very short entries would match everywhere.
        .filter(|l| l.chars().count() >= 4)
        .map(str::to_lowercase)
        .collect())
}

/// The built-in secret and PII scan of one text. Pure, so the self-tests feed it synthetic values.
pub fn scan_secrets(text: &str, denylist: &[String]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        // The owner's private identifiers count on every line: an allow marker never hides them.
        if !denylist.is_empty() {
            let lower = line.to_lowercase();
            for id in denylist {
                if let Some(at) = lower.find(id.as_str()) {
                    hits.push(Hit { rule: "private-identifier", line: n, col: at + 1, digest: None });
                }
            }
        }
        if ALLOW_MARKERS.iter().any(|m| line.contains(m)) {
            continue;
        }
        let b = line.as_bytes();
        for r in TOKEN_RULES {
            for p in r.prefixes {
                for at in find_all(line, p) {
                    if at > 0 && is_word(b[at - 1]) {
                        continue;
                    }
                    let tail = &b[at + p.len()..];
                    let len = tail.iter().take_while(|c| (r.class)(**c)).count();
                    if len >= r.min && r.exact.is_none_or(|e| len == e) {
                        hits.push(Hit::new(r.rule, n, at + 1, &b[at..at + p.len() + len]));
                    }
                }
            }
        }
        for at in find_all(line, "eyJ") {
            if (at == 0 || !is_base64url(b[at - 1]))
                && let Some(len) = jwt_len(&b[at..])
            {
                hits.push(Hit::new("jwt", n, at + 1, &b[at..at + len]));
            }
        }
        for at in find_all(line, "0000") {
            if let Some(col) = udid_at(b, at) {
                hits.push(Hit::new("udid", n, col, &b[at..at + 25]));
            }
        }
    }
    hits.extend(private_keys(text));
    hits.sort_by_key(|h| (h.line, h.col));
    hits.dedup();
    hits
}

/// A token-shaped secret: a fixed prefix followed by a run of characters of one class.
struct TokenRule {
    rule: &'static str,
    prefixes: &'static [&'static str],
    /// The least length of the run after the prefix.
    min: usize,
    /// The exact length, when the format fixes it (a longer run is something else).
    exact: Option<usize>,
    class: fn(u8) -> bool,
}

const TOKEN_RULES: &[TokenRule] = &[
    TokenRule {
        rule: "aws-access-key-id",
        prefixes: &["AKIA", "ASIA"],
        min: 16,
        exact: Some(16),
        class: is_upper_alnum,
    },
    TokenRule {
        rule: "github-token",
        prefixes: &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"],
        min: 36,
        exact: None,
        class: is_alnum,
    },
    TokenRule { rule: "github-fine-grained-token", prefixes: &["github_pat_"], min: 22, exact: None, class: is_word },
    TokenRule {
        rule: "slack-token",
        prefixes: &["xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-"],
        min: 10,
        exact: None,
        class: is_slack,
    },
    TokenRule { rule: "google-api-key", prefixes: &["AIza"], min: 35, exact: Some(35), class: is_base64url },
];

fn is_upper_alnum(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}
fn is_alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}
fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}
fn is_slack(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-'
}
fn is_base64url(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}
fn is_base64(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'='
}

fn find_all<'a>(hay: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    hay.match_indices(needle).map(|(i, _)| i)
}

/// `eyJ<≥10>.eyJ<≥10>.<≥10>` in base64url: a JSON Web Token (ASC tokens are ES256 JWTs). Its length, if one
/// starts at `b`.
fn jwt_len(b: &[u8]) -> Option<usize> {
    let mut parts = Vec::new();
    let mut i = 0;
    for k in 0..3 {
        let start = i;
        while i < b.len() && is_base64url(b[i]) {
            i += 1;
        }
        parts.push(&b[start..i]);
        if k < 2 {
            if i < b.len() && b[i] == b'.' {
                i += 1;
            } else {
                return None;
            }
        }
    }
    (parts[0].starts_with(b"eyJ") && parts[1].starts_with(b"eyJ") && parts.iter().all(|p| p.len() >= 10)).then_some(i)
}

/// A modern UDID `0000XXXX-XXXXXXXXXXXXXXXX` (chip id, ECID) at `at`, as a 1-based column. Two synthetic
/// forms pass: an all-zero ECID (the protocol tests' `00008101-0000000000000000`) and chip id 0000, which no
/// Apple chip has (the testkit's scenario devices, `00000000-0000000000000001`).
fn udid_at(b: &[u8], at: usize) -> Option<usize> {
    if at > 0 && b[at - 1].is_ascii_hexdigit() {
        return None;
    }
    let s = b.get(at..at + 25)?;
    let shaped = s[..8].iter().all(u8::is_ascii_hexdigit) && s[8] == b'-' && s[9..].iter().all(u8::is_ascii_hexdigit);
    let bounded = b.get(at + 25).is_none_or(|c| !c.is_ascii_hexdigit());
    let synthetic = s[9..].iter().all(|c| *c == b'0') || &s[..8] == b"00000000";
    (shaped && bounded && !synthetic).then_some(at + 1)
}

/// Private-key blocks, the way gitleaks' `private-key` rule sees them: `-----BEGIN`, up to 100 characters of
/// `[ A-Za-z0-9_-]`, `PRIVATE KEY` (PEM) or `PRIVATE KEY BLOCK` (PGP armour), `-----`, then a body of at least 64
/// characters (64 of them base64) up to the first `KEY-----` or `KEY BLOCK-----`. The body may span lines or sit
/// on one line with `\n` escapes (a key in a string literal). A bare header in documentation or a test is not a
/// key. An allow marker on the line where the block starts suppresses it, as gitleaks does.
fn private_keys(text: &str) -> Vec<Hit> {
    let begin = concat!("-----", "BEGIN");
    let bytes = text.as_bytes();
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(off) = text[from..].find(begin) {
        let at = from + off;
        from = at + begin.len();
        let Some(header_end) = private_key_header_end(bytes, at + begin.len()) else { continue };
        let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
        let line_end = text[at..].find('\n').map_or(text.len(), |e| at + e);
        if ALLOW_MARKERS.iter().any(|m| text[line_start..line_end].contains(m)) {
            continue;
        }
        let Some(end) = private_key_footer_end(bytes, header_end) else { continue };
        let body = &bytes[header_end..end];
        if body.iter().filter(|c| is_base64(**c)).count() < 64 {
            continue;
        }
        let line = text[..at].matches('\n').count() + 1;
        hits.push(Hit::new("private-key", line, at - line_start + 1, &bytes[at..end]));
        from = end;
    }
    hits
}

/// After `-----BEGIN` at `i`: the end of `<label>PRIVATE KEY[ BLOCK]-----`, if that header follows.
fn private_key_header_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    while j <= i + 100 && j < b.len() {
        let rest = &b[j..];
        if rest.len() >= 11 && rest[..11].eq_ignore_ascii_case(b"PRIVATE KEY") {
            let mut k = j + 11;
            if b.get(k..k + 6).is_some_and(|s| s.eq_ignore_ascii_case(b" BLOCK")) {
                k += 6;
            }
            return b.get(k..k + 5).is_some_and(|s| s == b"-----").then_some(k + 5);
        }
        let c = b[j];
        if !(c.is_ascii_alphanumeric() || c == b' ' || c == b'_' || c == b'-') {
            return None;
        }
        j += 1;
    }
    None
}

/// The end of the first `KEY-----` or `KEY BLOCK-----` at least 64 bytes after `start`, within 64 KiB.
fn private_key_footer_end(b: &[u8], start: usize) -> Option<usize> {
    let limit = b.len().min(start + 64 * 1024);
    let mut j = start + 64;
    while j < limit {
        let rest = &b[j..limit];
        if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"KEY") {
            let mut k = j + 3;
            if b.get(k..k + 6).is_some_and(|s| s.eq_ignore_ascii_case(b" BLOCK")) {
                k += 6;
            }
            if b.get(k..k + 5).is_some_and(|s| s == b"-----") {
                return Some(k + 5);
            }
        }
        j += 1;
    }
    None
}

// ---- public terms ------------------------------------------------------------------------------------------

/// SHA-256 digests (lowercase hex) of the private track's identifying terms: lowercased single tokens and
/// space-joined token pairs (ARCHITECTURE §19 [D 91]). The plaintext list lives in the private repository; the
/// lead produces these with `cargo xtask guard --hash-terms < list` and pastes them here. Until then the check
/// has nothing to compare and says so.
pub const PUBLIC_TERM_DIGESTS: &[&str] = &[];

/// The only files that may mention the private track. Q34 is answered (decisions.md keeps one neutral line, the
/// details live in the private repository), so `docs/decisions.md` left the list; the research file stays until
/// PLAN §5.9 item 4 decides it. The list must be empty before the repository goes public (PLAN §5.9 item 1).
pub const PUBLIC_TERMS_ALLOWLIST: &[&str] = &["docs/research/macos-on-x86-feasibility-2026-10.md"];

/// The digest of one term, normalized the way the scan tokenizes text: lowercased tokens joined by one space.
pub fn term_digest(term: &str) -> [u8; 32] {
    let tokens: Vec<String> = tokens(term).map(|(t, _)| t).collect();
    Sha256::digest(tokens.join(" ").as_bytes()).into()
}

/// `--hash-terms`: the digests of the terms in `text`, one per line (`#` comments and blank lines skipped). The
/// scan compares single tokens and adjacent token pairs only (ARCHITECTURE §19 [D 91]), so a line with no token
/// or with three or more is refused, naming its line number but never the term: such a digest could never match.
pub fn hash_terms(text: &str) -> Result<Vec<[u8; 32]>> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let term = line.trim();
        if term.is_empty() || term.starts_with('#') {
            continue;
        }
        let n = tokens(term).count();
        anyhow::ensure!(
            n == 1 || n == 2,
            "line {}: the term has {n} tokens, but the guard compares single tokens and adjacent pairs only \
             (ARCHITECTURE §19); split it into tokens or pairs",
            i + 1
        );
        out.push(term_digest(term));
    }
    Ok(out)
}

/// Lowercased alphanumeric tokens with their 1-based line numbers.
fn tokens(text: &str) -> impl Iterator<Item = (String, usize)> + '_ {
    let mut line = 1;
    let mut cur = String::new();
    let mut cur_line = 1;
    let mut chars = text.chars().chain(std::iter::once('\n'));
    std::iter::from_fn(move || {
        for c in chars.by_ref() {
            if c.is_alphanumeric() {
                if cur.is_empty() {
                    cur_line = line;
                }
                cur.extend(c.to_lowercase());
            } else {
                if c == '\n' {
                    line += 1;
                }
                if !cur.is_empty() {
                    return Some((std::mem::take(&mut cur), cur_line));
                }
            }
        }
        None
    })
}

fn read_digests(path: &Path) -> Result<Vec<[u8; 32]>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(parse_digest)
        .collect::<Result<_>>()
        .with_context(|| format!("parsing {}", path.display()))
}

fn parse_digest(hexstr: &str) -> Result<[u8; 32]> {
    anyhow::ensure!(hexstr.len() == 64, "a SHA-256 digest has 64 hex digits: {hexstr:?}");
    let mut out = [0u8; 32];
    for (i, chunk) in hexstr.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk)?;
        out[i] = u8::from_str_radix(s, 16).with_context(|| format!("not hex: {hexstr:?}"))?;
    }
    Ok(out)
}

/// Positions where a text contains a term of `digests`: `(line, digest)` for every token or token pair whose
/// digest is listed.
pub fn find_terms(text: &str, digests: &HashSet<[u8; 32]>) -> Vec<(usize, [u8; 32])> {
    let mut out = Vec::new();
    let mut prev: Option<(String, usize)> = None;
    for (tok, line) in tokens(text) {
        let d: [u8; 32] = Sha256::digest(tok.as_bytes()).into();
        if digests.contains(&d) {
            out.push((line, d));
        }
        if let Some((p, pline)) = &prev {
            let pair: [u8; 32] = Sha256::digest(format!("{p} {tok}").as_bytes()).into();
            if digests.contains(&pair) {
                out.push((*pline, pair));
            }
        }
        prev = Some((tok, line));
    }
    out
}

fn public_terms(root: &Path, files: &[String], opts: &Options) -> Result<CheckResult> {
    let mut digests: HashSet<[u8; 32]> = HashSet::new();
    for h in PUBLIC_TERM_DIGESTS {
        digests.insert(parse_digest(h).context("PUBLIC_TERM_DIGESTS")?);
    }
    digests.extend(opts.extra_digests.iter().copied());
    let mut r = CheckResult::default();
    if digests.is_empty() {
        r.note = Some("no term digests compiled in yet (the lead supplies them, PLAN W8); nothing compared".into());
        println!("warning: guard public-terms: {}", r.note.as_deref().unwrap_or_default());
        return Ok(r);
    }
    let mut scanned = 0;
    for rel in files {
        if PUBLIC_TERMS_ALLOWLIST.contains(&rel.as_str()) {
            continue;
        }
        for (_, d) in find_terms(rel, &digests) {
            r.findings.push(format!("{rel}: the file name contains a private-track term (digest {}…)", &hex(&d)[..12]));
        }
        let Some(text) = read_text(&root.join(rel)) else { continue };
        scanned += 1;
        for (line, d) in find_terms(&text, &digests) {
            r.findings.push(format!("{rel}:{line}: private-track term (digest {}…)", &hex(&d)[..12]));
        }
    }
    r.note = Some(format!(
        "{scanned} files, {} digests, {} allowlisted file(s)",
        digests.len(),
        PUBLIC_TERMS_ALLOWLIST.len()
    ));
    Ok(r)
}

// ---- external tools ----------------------------------------------------------------------------------------

/// Runs a tree-wide external tool in the repository root.
fn external_tree(opts: &Options, tool: &str, root: &Path, args: impl FnOnce(&mut Command)) -> Result<CheckResult> {
    if !opts.artifacts.is_empty() {
        return Ok(CheckResult::skip("artifact scans run apple-files and secrets only"));
    }
    if opts.builtin_only {
        return Ok(CheckResult::skip("external tools disabled"));
    }
    if !crate::ci::has_program(tool) {
        return Ok(CheckResult::missing(tool));
    }
    let mut cmd = Command::new(tool);
    cmd.current_dir(root);
    args(&mut cmd);
    let out = cmd.output().with_context(|| format!("running {tool}"))?;
    let mut r = CheckResult::default();
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        r.findings.extend(text.lines().filter(|l| !l.trim().is_empty()).map(str::to_owned).take(200));
        if r.findings.is_empty() {
            r.findings.push(format!("{tool} exited with {}", out.status));
        }
    }
    Ok(r)
}

/// `reuse lint` over exactly the files the other guards see (tracked, plus untracked and not ignored). reuse
/// itself descends into some git-ignored directories, such as the Node harness's `node_modules`, so it runs on a
/// temporary copy of that file set (about 3 MB).
fn reuse(root: &Path, files: Option<&[String]>, opts: &Options) -> Result<CheckResult> {
    let Some(files) = files else {
        return Ok(CheckResult::skip("artifact scans run apple-files and secrets only"));
    };
    if opts.builtin_only {
        return Ok(CheckResult::skip("external tools disabled"));
    }
    if !crate::ci::has_program("reuse") {
        return Ok(CheckResult::missing("reuse"));
    }
    let copy = ScratchDir::new("russet-reuse")?;
    for rel in files {
        let to = copy.0.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(root.join(rel), &to).with_context(|| format!("copying {rel}"))?;
    }
    let mut r = external_tree(opts, "reuse", &copy.0, |cmd| {
        cmd.arg("--root").arg(&copy.0).arg("lint");
    })?;
    let prefix = format!("{}/", copy.0.display());
    for f in &mut r.findings {
        *f = f.replace(&prefix, "");
    }
    r.note = Some(format!("{} files", files.len()));
    Ok(r)
}

/// A private temporary directory, removed on drop (xtask has no tempfile dependency outside tests).
struct ScratchDir(PathBuf);

impl ScratchDir {
    /// A new directory under the system temp dir, private to the user (0700): gitleaks' report in it holds the
    /// matched values.
    fn new(prefix: &str) -> Result<Self> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The variables zizmor reads its GitHub token from.
const ZIZMOR_TOKEN_ENV: &[&str] = &["GH_TOKEN", "GITHUB_TOKEN", "ZIZMOR_GITHUB_TOKEN"];

fn zizmor(root: &Path, opts: &Options) -> Result<CheckResult> {
    if !root.join(".github").is_dir() {
        return Ok(CheckResult::skip("no .github directory"));
    }
    if opts.zizmor_online && !ZIZMOR_TOKEN_ENV.iter().any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty())) {
        bail!("--zizmor-online needs a GitHub token in GH_TOKEN (CI: the job's github.token)");
    }
    // Medium severity and above fail (QA-03). Offline by default: no GitHub API calls. Online, zizmor also checks
    // that every pinned SHA belongs to its action's repository (impostor-commit) and is not a known-vulnerable
    // version.
    let mut r = external_tree(opts, "zizmor", root, |cmd| {
        if !opts.zizmor_online {
            cmd.arg("--offline");
        }
        cmd.args(["--min-severity", "medium", "--no-progress", ".github"]);
    })?;
    if r.skipped.is_none() {
        r.note = Some(if opts.zizmor_online { "online and offline audits" } else { "offline audits" }.into());
    }
    Ok(r)
}

// ---- commits -----------------------------------------------------------------------------------------------

/// The Conventional Commits types this repository uses.
const COMMIT_TYPES: &[&str] =
    &["feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitRule {
    Conventional,
    SignedOff,
}

/// Whether a subject follows Conventional Commits 1.0: `type(scope)!: description`.
pub fn is_conventional(subject: &str) -> bool {
    let Some((head, desc)) = subject.split_once(": ") else { return false };
    if desc.trim().is_empty() {
        return false;
    }
    let head = head.strip_suffix('!').unwrap_or(head);
    let (ty, scope) = match head.split_once('(') {
        Some((ty, rest)) => match rest.strip_suffix(')') {
            Some(scope) => (ty, Some(scope)),
            None => return false,
        },
        None => (head, None),
    };
    COMMIT_TYPES.contains(&ty)
        && scope.is_none_or(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || " ,._/-".contains(c)))
}

fn commit_check(root: &Path, opts: &Options, rule: CommitRule) -> Result<CheckResult> {
    if !opts.artifacts.is_empty() {
        return Ok(CheckResult::skip("artifact scans run apple-files and secrets only"));
    }
    let base = match &opts.base {
        Some(b) => b.clone(),
        None => {
            let candidates = ["main", "origin/main"];
            match candidates.iter().find(|c| git_ok(root, &["rev-parse", "--verify", "--quiet", c])) {
                Some(c) => c.to_string(),
                None => return Ok(CheckResult::skip("no main or origin/main to compare with")),
            }
        }
    };
    let Some(mb) = git_out(root, &["merge-base", &base, "HEAD"]) else {
        return Ok(CheckResult::skip(format!("no merge base with {base}")));
    };
    let range = format!("{}..HEAD", mb.trim());
    let log = git_out(root, &["log", "--no-merges", "--format=%H%x1f%s%x1f%b%x1e", &range])
        .context("git log of the branch")?;
    let mut r = CheckResult::default();
    let mut count = 0;
    for rec in log.split('\u{1e}').map(str::trim).filter(|s| !s.is_empty()) {
        let mut parts = rec.splitn(3, '\u{1f}');
        let (sha, subject, body) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        count += 1;
        let short = sha.get(..10).unwrap_or(sha);
        match rule {
            CommitRule::Conventional if !is_conventional(subject) => {
                r.findings.push(format!("{short}: not a Conventional Commit: {subject:?}"));
            }
            CommitRule::SignedOff if !body.lines().any(|l| l.trim_start().starts_with("Signed-off-by: ")) => {
                r.findings.push(format!("{short}: no Signed-off-by trailer (DCO, CONTRIBUTING.md)"));
            }
            _ => {}
        }
    }
    r.note = Some(format!("{count} commit(s) since {base}"));
    Ok(r)
}

fn git_out(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_ok(root: &Path, args: &[&str]) -> bool {
    Command::new("git").arg("-C").arg(root).args(args).output().is_ok_and(|o| o.status.success())
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use super::*;

    /// A temporary git repository with a committer identity.
    struct Repo {
        dir: tempfile::TempDir,
    }

    impl Repo {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let r = Self { dir };
            r.git(&["init", "-q", "-b", "main"]);
            r
        }
        fn path(&self) -> &Path {
            self.dir.path()
        }
        fn write(&self, rel: &str, bytes: &[u8]) {
            let p = self.path().join(rel);
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(p, bytes).expect("write");
        }
        fn git(&self, args: &[&str]) {
            let ok = Command::new("git")
                .arg("-C")
                .arg(self.path())
                .args([
                    "-c",
                    "user.name=Russet Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .expect("git");
            assert!(ok.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ok.stderr));
        }
        fn commit(&self, msg: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "--allow-empty", "-m", msg]);
        }
    }

    /// Findings grouped by check.
    fn by_check(findings: &[String]) -> BTreeMap<String, Vec<String>> {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for f in findings {
            let (check, rest) = f.split_once(": ").unwrap_or(("?", f));
            m.entry(check.to_string()).or_default().push(rest.to_string());
        }
        m
    }

    fn builtin(extra_digests: Vec<[u8; 32]>) -> Options {
        Options { extra_digests, no_default_denylist: true, builtin_only: true, ..Options::default() }
    }

    /// A synthetic PEM private key: random-looking base64 made at test time, never a real key.
    fn fake_pem() -> String {
        let body: String = (0..4).map(|i| format!("{}\n", "QUJD".repeat(16).replace('Q', &i.to_string()))).collect();
        format!("{}PRIVATE KEY-----\n{body}{}PRIVATE KEY-----\n", concat!("-----", "BEGIN "), concat!("-----", "END "))
    }

    /// A synthetic PGP armoured private key: armour headers around made-up base64, never a real key.
    fn fake_pgp() -> String {
        let body: String = (0..4).map(|i| format!("{}\n", "WFla".repeat(16).replace('W', &i.to_string()))).collect();
        format!(
            "{}PGP PRIVATE KEY BLOCK-----\nComment: synthetic\n\n{body}=AbCd\n{}PGP PRIVATE KEY BLOCK-----\n",
            concat!("-----", "BEGIN "),
            concat!("-----", "END ")
        )
    }

    /// Rule, line and column of each hit.
    fn spots(hits: &[Hit]) -> Vec<(&'static str, usize, usize)> {
        hits.iter().map(|h| (h.rule, h.line, h.col)).collect()
    }

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    /// PLAN W8 test: seeded bad fixtures are all caught, in under 30 s.
    #[test]
    fn seeded_bad_fixtures_are_all_caught_quickly() {
        let start = Instant::now();
        let repo = Repo::new();
        // apple-files
        repo.write("bin/probe", &[0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1, 0, 0, 0, 0, 2, 0, 0, 0]);
        repo.write("keys/dist.p12", b"not really a p12");
        // provenance: an unlisted xar, a listed one whose bytes changed, and a good one
        let good = b"xar!\x00\x1c\x00\x01good";
        // The licence line is spelled in pieces so `reuse lint` does not read it as this file's.
        let script = concat!("#!/bin/sh\n# SPDX-", "License-Identifier: 0BSD\necho generated\n");
        repo.write("tests/fixtures/xip/gen/make.sh", script.as_bytes());
        repo.write("tests/fixtures/xip/unlisted.xar", b"xar!\x00\x1c\x00\x01unlisted");
        repo.write("tests/fixtures/xip/changed.xar", b"xar!\x00\x1c\x00\x01changed later");
        repo.write("tests/fixtures/xip/good.xar", good);
        let manifest = format!(
            "schema = 1\n\n[[file]]\npath = \"changed.xar\"\nsha256 = \"{}\"\ngenerator = \"tests/fixtures/xip/gen/make.sh\"\n\n\
             [[file]]\npath = \"good.xar\"\nsha256 = \"{}\"\ngenerator = \"tests/fixtures/xip/gen/make.sh\"\n",
            sha(b"xar!\x00\x1c\x00\x01original"),
            sha(good)
        );
        repo.write("tests/fixtures/xip/PROVENANCE.toml", manifest.as_bytes());
        // secrets
        repo.write("notes/key.txt", fake_pem().as_bytes());
        repo.write("notes/pgp.asc", fake_pgp().as_bytes());
        repo.write("src/escaped.rs", format!("let key = {:?};\n", fake_pem()).as_bytes());
        repo.write("notes/token.md", format!("token: {}{}\n", "ghp_", "a1B2".repeat(9)).as_bytes());
        repo.write("notes/aws.md", format!("id = {}{}\n", "AKIA", "Z7".repeat(8)).as_bytes());
        repo.write(
            "notes/udid.md",
            format!(
                "udid {}-{}\nfake 00008101-{}\nscenario 00000000-{}\n",
                "00008101",
                "001A2B3C4D5E6F70",
                "0".repeat(16),
                "0000000000000001"
            )
            .as_bytes(),
        );
        // public terms: a canary term outside the allowlist, and the same term inside it; decisions.md left the
        // allowlist once Q34 was answered
        let canary = "zebracanary quietword";
        repo.write("docs/leak.md", format!("We must not say {canary} here.\n").as_bytes());
        repo.write("docs/decisions.md", format!("The owner answered Q34; {canary}.\n").as_bytes());
        repo.write(PUBLIC_TERMS_ALLOWLIST[0], format!("The research may say {canary} (Q34).\n").as_bytes());
        repo.commit("chore: seed");

        let opts = builtin(vec![term_digest(canary)]);
        let checks = [Check::AppleFiles, Check::Secrets, Check::PublicTerms];
        let summary = run_checks(repo.path(), &checks, &opts).expect("guard");
        let found = by_check(&summary.findings);
        let joined = |c: &str| found.get(c).map(|v| v.join("\n")).unwrap_or_default();

        let apple = joined("apple-files");
        assert!(apple.contains("bin/probe: Mach-O magic"), "{apple}");
        assert!(apple.contains("keys/dist.p12: Apple file extension"), "{apple}");
        assert!(apple.contains("tests/fixtures/xip/unlisted.xar: xar magic (provenance: not listed"), "{apple}");
        assert!(apple.contains("tests/fixtures/xip/changed.xar: xar magic (provenance: its SHA-256"), "{apple}");
        assert!(!apple.contains("good.xar"), "a listed file with a matching hash passes: {apple}");

        let secrets = joined("secrets");
        assert!(secrets.contains("notes/key.txt:1:1: private-key"), "{secrets}");
        assert!(secrets.contains("notes/pgp.asc:1:1: private-key"), "a PGP armoured key: {secrets}");
        assert!(secrets.contains("src/escaped.rs:1:12: private-key"), "a key with \\n escapes: {secrets}");
        assert!(secrets.contains("notes/token.md:1:8: github-token"), "{secrets}");
        assert!(secrets.contains("notes/aws.md:1:6: aws-access-key-id"), "{secrets}");
        assert!(secrets.contains("notes/udid.md:1:6: udid"), "{secrets}");
        assert!(!secrets.contains("notes/udid.md:2"), "an all-zero ECID is a fake: {secrets}");
        assert!(!secrets.contains("notes/udid.md:3"), "chip id 0000 is a fake: {secrets}");
        assert!(!secrets.contains("a1B2"), "values are never printed");

        let terms = joined("public-terms");
        assert!(terms.contains("docs/leak.md:1: private-track term"), "{terms}");
        assert!(terms.contains("docs/decisions.md:1: private-track term"), "Q34 is answered: {terms}");
        assert!(!terms.contains("macos-on-x86"), "the allowlist holds: {terms}");
        assert!(!terms.contains(canary), "the term itself is never printed");

        assert_eq!(summary.failed, ["apple-files", "secrets", "public-terms"]);
        assert!(start.elapsed() < Duration::from_secs(30), "took {:?}", start.elapsed());
    }

    #[test]
    fn a_generator_must_be_committed() {
        let repo = Repo::new();
        let bytes = b"pbzx\0\0\0\0\x01\0\0\0";
        repo.write("fuzz/russet-xar/corpus/seed1", bytes);
        let manifest = format!(
            "schema = 1\n[[file]]\npath = \"seed1\"\nsha256 = \"{}\"\ngenerator = \"fuzz/russet-xar/gen.sh\"\n",
            sha(bytes)
        );
        repo.write("fuzz/russet-xar/corpus/PROVENANCE.toml", manifest.as_bytes());
        repo.commit("test: seed without a generator");
        let v = provenance_verdict(repo.path(), "fuzz/russet-xar/corpus/seed1");
        assert!(matches!(&v, Verdict::Rejected(w) if w.contains("does not exist")), "{v:?}");
        repo.write("fuzz/russet-xar/gen.sh", b"#!/bin/sh\n");
        let v = provenance_verdict(repo.path(), "fuzz/russet-xar/corpus/seed1");
        assert!(matches!(&v, Verdict::Rejected(w) if w.contains("not committed")), "{v:?}");
        repo.commit("test: add the generator");
        assert_eq!(provenance_verdict(repo.path(), "fuzz/russet-xar/corpus/seed1"), Verdict::Allowed);
        let finding = Finding { path: "fuzz/russet-xar/corpus/seed1".into(), reasons: vec!["pbzx magic"] };
        assert!(provenance_allows(repo.path(), &finding));
        // Only the compiled-in roots count, whatever PROVENANCE.toml files exist elsewhere.
        assert_eq!(provenance_verdict(repo.path(), "tests/fixtures/other/seed1"), Verdict::NotUnderRoot);
    }

    #[test]
    fn artifact_scans_honour_the_upload_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = dir.path();
        let macho = [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1];
        fs::create_dir_all(d.join("RussetProbe.app")).expect("mkdir");
        fs::write(d.join("RussetProbe.app/RussetProbe"), macho).expect("write");
        fs::write(d.join("libfoo.dylib"), macho).expect("write");
        fs::write(d.join("embedded.mobileprovision"), b"x").expect("write");
        let allow = r#"
            schema = 1
            [[allow]]
            workflow = "apple-release.yml"
            job = "release"
            paths = ["RussetProbe.app/RussetProbe", "embedded.mobileprovision"]
        "#;
        let upload = UploadAllow::parse(allow, "apple-release.yml", "release").expect("parse");
        let opts = Options { artifacts: vec![d.to_path_buf()], upload: Some(upload), ..builtin(Vec::new()) };
        let s = run_checks(Path::new("."), &[Check::AppleFiles], &opts).expect("guard");
        let text = s.findings.join("\n");
        assert!(!text.contains("RussetProbe.app/RussetProbe"), "our own binary may be uploaded: {text}");
        assert!(text.contains("libfoo.dylib: Mach-O magic"), "{text}");
        assert!(
            text.contains("embedded.mobileprovision: Apple file extension"),
            "only Mach-O is ever admitted: {text}"
        );
        // Another job of the same workflow gets nothing.
        let other = UploadAllow::parse(allow, "apple-release.yml", "lint").expect("parse");
        assert!(!other.admits("RussetProbe.app/RussetProbe", &["Mach-O magic"]));
    }

    /// A missing path fails by default; the upload-safe action passes `if-no-files-found` (warn, ignore), which is
    /// safe because upload-artifact uploads nothing for a plain path that does not exist either.
    #[test]
    fn a_missing_artifact_path_follows_if_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |if_missing| {
            let opts = Options { artifacts: vec![dir.path().join("nope")], if_missing, ..builtin(Vec::new()) };
            run_checks(Path::new("."), &[Check::AppleFiles, Check::Secrets], &opts).expect("guard")
        };
        assert_eq!(run(IfMissing::Error).failed, ["apple-files", "secrets"]);
        assert!(run(IfMissing::Warn).failed.is_empty());
        assert!(run(IfMissing::Ignore).failed.is_empty());
    }

    /// upload-artifact expands globs, exclusions, `~` and several lines; the guard refuses them instead of
    /// scanning something else than what would be uploaded.
    #[test]
    fn upload_paths_must_be_plain() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("probe.dylib"), [0xcf, 0xfa, 0xed, 0xfe]).expect("write");
        let glob = format!("{}/*.dylib", dir.path().display());
        let lines = format!("{}\n{}", dir.path().display(), dir.path().display());
        for bad in [glob.as_str(), lines.as_str(), "!target/x", "~/x", "out[1]", "out{a,b}", "x?", " "] {
            assert!(plain_path(Path::new(bad)).is_err(), "{bad:?}");
            let opts =
                Options { artifacts: vec![PathBuf::from(bad)], if_missing: IfMissing::Ignore, ..builtin(Vec::new()) };
            let s = run_checks(Path::new("."), &[Check::AppleFiles], &opts).expect("guard");
            assert_eq!(s.failed, ["apple-files"], "{bad:?} must fail, not scan nothing");
        }
        plain_path(dir.path()).expect("a plain directory");
        plain_path(Path::new("target/nextest/ci/junit.xml")).expect("a plain file");
    }

    /// upload-artifact follows symlinks, so the guard scans their targets under the link's own path.
    #[cfg(unix)]
    #[test]
    fn artifact_scans_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().expect("tempdir");
        fs::write(outside.path().join("libapple.dylib"), [0xcf, 0xfa, 0xed, 0xfe, 7, 0, 0, 1]).expect("write");
        fs::create_dir(outside.path().join("sub")).expect("mkdir");
        fs::write(outside.path().join("sub/Foo.mobileprovision"), b"x").expect("write");
        let up = tempfile::tempdir().expect("tempdir");
        let d = up.path();
        fs::write(d.join("report.xml"), b"<testsuites/>").expect("write");
        symlink(outside.path().join("libapple.dylib"), d.join("innocent.txt")).expect("link");
        symlink(outside.path().join("sub"), d.join("linked-dir")).expect("link");
        symlink(d, d.join("loop")).expect("link");
        symlink(d.join("gone"), d.join("dangling")).expect("link");
        let opts = Options { artifacts: vec![d.to_path_buf()], ..builtin(Vec::new()) };
        let s = run_checks(Path::new("."), &[Check::AppleFiles], &opts).expect("guard");
        let text = s.findings.join("\n");
        assert!(text.contains("innocent.txt: Mach-O magic"), "a link to a Mach-O file: {text}");
        assert!(text.contains("linked-dir/Foo.mobileprovision: Apple file extension"), "a linked directory: {text}");
        assert_eq!(s.failed, ["apple-files"]);
        // The upload allowlist matches the path the upload uses, the link's.
        let allow = UploadAllow::parse(
            "schema = 1\n[[allow]]\nworkflow = \"w.yml\"\njob = \"j\"\npaths = [\"innocent.txt\"]\n",
            "w.yml",
            "j",
        )
        .expect("parse");
        assert!(allow.admits("innocent.txt", &["Mach-O magic"]));
        // A link to a single file given directly is scanned too.
        let single = Options { artifacts: vec![d.join("innocent.txt")], ..builtin(Vec::new()) };
        assert_eq!(run_checks(Path::new("."), &[Check::AppleFiles], &single).expect("guard").failed, ["apple-files"]);
    }

    #[test]
    fn the_secret_rules_match_shapes_not_mentions() {
        let none: &[String] = &[];
        // Mentions, prefixes and documentation are not secrets.
        let docs = concat!(
            "AKIA alone; ghp_ alone; the -----",
            "BEGIN PRIVATE KEY----- header line of a .p8 file;\n",
            "xoxb- tokens; eyJhbGciOi.short; 00008101-… udid; sha 0000aaaa-bbbb\n"
        );
        assert_eq!(scan_secrets(docs, none), Vec::<Hit>::new());
        let jwt = format!("Bearer {}.{}.{}", "eyJhbGciOiJFUzI1NiJ9", "eyJpc3MiOiJ0ZXN0In0", "c2lnbmF0dXJlcGFydA");
        assert_eq!(spots(&scan_secrets(&jwt, none)), [("jwt", 1, 8)]);
        // The digest is of the token alone, which is what gitleaks reports as the secret.
        assert_eq!(scan_secrets(&jwt, none)[0].digest, Some(value_digest("jwt", &jwt.as_bytes()[7..])));
        let slack = format!("{}{}", "xoxb-", "1234567890-abc");
        assert_eq!(scan_secrets(&slack, none)[0].rule, "slack-token");
        let google = format!("k={}{}", "AIza", "S".repeat(35));
        assert_eq!(scan_secrets(&google, none)[0].rule, "google-api-key");
        // Inline suppression, for reviewed false positives only.
        let allowed = format!("{}{} # gitleaks:allow", "AKIA", "Q".repeat(16));
        assert!(scan_secrets(&allowed, none).is_empty());
        assert_eq!(scan_secrets(&fake_pem(), none).len(), 1);
        assert!(scan_secrets(&format!("// gitleaks:allow (test key) {}", fake_pem()), none).is_empty());
        // A longer run of key characters is not an AWS key id.
        let longer = format!("{}{}", "AKIA", "Q".repeat(20));
        assert!(scan_secrets(&longer, none).is_empty());
        // PGP armour and a PEM block written on one line with \n escapes are keys too; a bare header is not.
        assert_eq!(spots(&scan_secrets(&fake_pgp(), none)), [("private-key", 1, 1)]);
        let escaped = format!("const K: &str = {:?};", fake_pem());
        assert_eq!(spots(&scan_secrets(&escaped, none)), [("private-key", 1, 18)]);
        let header_only = format!("{}PGP PRIVATE KEY BLOCK-----\n", concat!("-----", "BEGIN "));
        assert!(scan_secrets(&header_only, none).is_empty());
        // UDIDs are compared upper-cased.
        let udid = format!("{}-{}", "00008101", "001a2b3c4d5e6f70");
        assert_eq!(scan_secrets(&udid, none)[0].digest, Some(value_digest("udid", udid.to_uppercase().as_bytes())));
    }

    #[test]
    fn the_private_denylist_matches_case_insensitively_and_is_never_echoed() {
        let deny = vec!["owner's phone".to_string()];
        let hits = scan_secrets("Connected to Owner's Phone today\n", &deny);
        assert_eq!(hits, vec![Hit { rule: "private-identifier", line: 1, col: 14, digest: None }]);
        // An allow marker hides the line from the secret rules, never from the denylist.
        let marked = format!("{}{} owner's phone # gitleaks:allow\n", "AKIA", "Q".repeat(16));
        assert_eq!(spots(&scan_secrets(&marked, &deny)), [("private-identifier", 1, 22)]);
    }

    #[test]
    fn the_synthetic_allowlist_admits_reviewed_values_only() {
        let repo = Repo::new();
        let synthetic = format!("{}-{}", "00008101", "0123456789ABCDEF");
        let other = format!("{}-{}", "00008101", "0FEDCBA987654321");
        repo.write("tests/devices.rs", format!("{synthetic}\n{}\n{other}\n", synthetic.to_lowercase()).as_bytes());
        repo.commit("test: two synthetic udids");
        let entry = format!(
            "schema = 1\n[[synthetic]]\nrule = \"udid\"\nsha256 = \"{}\"\nowner = \"W4\"\nreason = \"a hand-typed test UDID\"\n",
            hex(&value_digest("udid", synthetic.as_bytes()))
        );
        repo.write(SYNTHETIC_ALLOWLIST, entry.as_bytes());
        let s = run_checks(repo.path(), &[Check::Secrets], &builtin(Vec::new())).expect("guard");
        let found = by_check(&s.findings);
        // Both spellings of the listed value pass; the unlisted one fails.
        assert_eq!(found["secrets"], ["tests/devices.rs:3:1: udid (value not shown)"], "{found:?}");
        // Suggestions carry the digest of what is left, for a reviewer to add.
        let suggest = Options { suggest_allowlist: true, ..builtin(Vec::new()) };
        let r = secrets(repo.path(), Some(&tree_files(repo.path()).expect("files")), &suggest).expect("secrets");
        assert_eq!(r.suggestions.len(), 1);
        assert!(r.suggestions[0].contains(&hex(&value_digest("udid", other.as_bytes()))), "{:?}", r.suggestions);
        // Entries need a valid digest, an owner and a reason.
        for bad in [
            "schema = 2\n",
            "schema = 1\n[[synthetic]]\nrule = \"udid\"\nsha256 = \"zz\"\nowner = \"W4\"\nreason = \"a reason here\"\n",
            &entry.replace("a hand-typed test UDID", "x"),
            &entry.replace("owner = \"W4\"", "owner = \"\""),
            &entry.replace("reason", "why"),
        ] {
            assert!(SyntheticAllowlist::parse(bad).is_err(), "{bad}");
        }
        // An explicit allowlist path must exist.
        let missing = Options { synthetic_allowlist: Some(repo.path().join("nope.toml")), ..builtin(Vec::new()) };
        assert!(run_checks(repo.path(), &[Check::Secrets], &missing).expect("guard").failed == ["secrets"]);
    }

    /// gitleaks' report is filtered by the digest of the secret it reports, and printed without it.
    #[test]
    fn gitleaks_reports_are_filtered_by_the_allowlist() {
        let listed = format!("{}{}", "ghp_", "Z9".repeat(18));
        let unlisted = format!("{}{}", "ghp_", "Y8".repeat(18));
        let report = serde_json::json!([
            {"RuleID": "github-pat", "File": "a.rs", "StartLine": 3, "Commit": "0123456789abcdef", "Secret": listed},
            {"RuleID": "github-pat", "File": "b.rs", "StartLine": 9, "Commit": "fedcba9876543210", "Secret": unlisted},
            {"RuleID": "jwt", "File": "/up/report.xml", "StartLine": 1, "Commit": "", "Secret": "eyJx"},
        ])
        .to_string();
        let allow = SyntheticAllowlist::parse(&format!(
            "schema = 1\n[[synthetic]]\nrule = \"github-pat\"\nsha256 = \"{}\"\nowner = \"lead\"\nreason = \"a test token\"\n",
            sha(listed.as_bytes())
        ))
        .expect("parse");
        let mut out = GitleaksOutcome::default();
        assert_eq!(filter_gitleaks_report(&report, &allow, &mut out).expect("filter"), 3);
        assert_eq!(out.admitted, 1);
        assert_eq!(
            out.findings,
            [
                "b.rs:9: github-pat (gitleaks, commit fedcba9876; value not shown)",
                "/up/report.xml:1: jwt (gitleaks; value not shown)"
            ]
        );
        assert!(!out.findings.concat().contains("Y8Y8"), "values are never printed");
        assert!(filter_gitleaks_report("not json", &allow, &mut out).is_err());
    }

    /// With gitleaks installed (CI installs it), a leak in an old commit fails even after the file is fixed, and
    /// a reviewed synthetic value passes in every commit.
    #[test]
    fn gitleaks_reads_the_branch_history() {
        if !crate::ci::has_program("gitleaks") {
            println!("gitleaks is not installed: skipped");
            return;
        }
        let repo = Repo::new();
        // gitleaks wants a token with entropy: 36 base62 characters derived from a hash at test time.
        let alnum = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let seed = Sha256::digest(b"russet guard test token");
        let body: String = (0..36).map(|i| alnum[usize::from(seed[i % 32] ^ (i as u8)) % 62] as char).collect();
        let token = format!("{}{body}", "ghp_");
        repo.write("src/lib.rs", format!("const T: &str = \"{token}\";\n").as_bytes());
        repo.commit("feat: a token");
        repo.write("src/lib.rs", b"const T: &str = \"\";\n");
        repo.commit("fix: drop the token");
        let opts = Options { no_default_denylist: true, ..Options::default() };
        let s = run_checks(repo.path(), &[Check::Secrets], &opts).expect("guard");
        let text = s.findings.join("\n");
        assert_eq!(s.failed, ["secrets"], "{text}");
        assert!(text.contains("src/lib.rs:1:") && text.contains("(gitleaks, commit "), "{text}");
        assert!(!text.contains(&body), "values are never printed: {text}");
        let entry = format!(
            "schema = 1\n[[synthetic]]\nrule = \"github-pat\"\nsha256 = \"{}\"\nowner = \"lead\"\nreason = \"a test token\"\n",
            sha(token.as_bytes())
        );
        repo.write(SYNTHETIC_ALLOWLIST, entry.as_bytes());
        let s = run_checks(repo.path(), &[Check::Secrets], &opts).expect("guard");
        assert!(s.failed.is_empty(), "{:?}", s.findings);
    }

    #[test]
    fn public_terms_hash_tokens_and_pairs() {
        let set: HashSet<[u8; 32]> = [term_digest("Alpha"), term_digest("beta GAMMA")].into();
        let text = "x alpha y\nbeta\ngamma z\nbetagamma\n";
        let found: Vec<usize> = find_terms(text, &set).into_iter().map(|(l, _)| l).collect();
        // `alpha` on line 1; the pair `beta gamma` across a line break starts on line 2.
        assert_eq!(found, [1, 2]);
        assert_eq!(hex(&term_digest("  Beta   gamma ")), hex(&term_digest("beta gamma")));
        assert!(parse_digest("zz").is_err());
        assert_eq!(parse_digest(&hex(&term_digest("x"))).expect("parse"), term_digest("x"));
        // --hash-terms takes tokens and pairs only: a longer term could never match, so it is refused, by line.
        let digests = hash_terms("# private list\nAlpha\n\nbeta GAMMA\n").expect("hash");
        assert_eq!(digests, [term_digest("alpha"), term_digest("beta gamma")]);
        let err = hash_terms("alpha\nalpha beta gamma\n").expect_err("three tokens").to_string();
        assert!(err.contains("line 2") && err.contains("3 tokens") && !err.contains("gamma"), "{err}");
        assert!(hash_terms("--\n").expect_err("no token").to_string().contains("0 tokens"));
    }

    #[test]
    fn conventional_commit_subjects() {
        for ok in ["feat: x", "fix(xtask): y", "feat(testkit, ci)!: z", "docs(contributing/ci): w", "revert: v"] {
            assert!(is_conventional(ok), "{ok}");
        }
        for bad in ["Add stuff", "feat:x", "feature: x", "fix(): y", "fix(scope: y", "feat: ", "Merge W1: x"] {
            assert!(!is_conventional(bad), "{bad}");
        }
    }

    #[test]
    fn commit_checks_cover_the_branch_only() {
        let repo = Repo::new();
        repo.commit("initial commit, not conventional (main's history is not checked)");
        repo.git(&["checkout", "-q", "-b", "ws/w08-test"]);
        repo.commit("feat(testkit): fine");
        repo.commit("Fix things");
        let opts = Options { base: Some("main".into()), ..builtin(Vec::new()) };
        let s = run_checks(repo.path(), &[Check::Commits, Check::Dco], &opts).expect("guard");
        let found = by_check(&s.findings);
        assert_eq!(found["commits"].len(), 1, "{found:?}");
        assert!(found["commits"][0].contains("Fix things"));
        assert_eq!(found["dco"].len(), 2, "{found:?}");
    }

    #[test]
    fn checks_parse_and_all_leaves_out_dco() {
        assert_eq!(Check::parse("check-no-apple").expect("alias"), Check::AppleFiles);
        assert!(Check::parse("everything").is_err());
        assert!(!Check::ALL.contains(&Check::Dco));
        assert!(PROVENANCE_ROOTS.iter().all(|r| r.ends_with('/')));
        assert_eq!(PUBLIC_TERMS_ALLOWLIST, ["docs/research/macos-on-x86-feasibility-2026-10.md"], "Q34 is answered");
        for d in PUBLIC_TERM_DIGESTS {
            parse_digest(d).expect("every compiled-in digest is 64 hex digits");
        }
    }

    /// The committed upload allowlist admits apple-release.yml's RussetProbe Mach-O outputs and nothing else.
    #[test]
    fn the_committed_upload_allowlist_names_only_russetprobe() {
        let file = crate::workstreams::repo_root().join(".github/upload-safe.toml");
        let release = UploadAllow::load(&file, "apple-release.yml", "release").expect("parse");
        assert!(release.admits("RussetProbe.app/RussetProbe", &["Mach-O magic"]));
        assert!(release.admits("out/RussetProbe.app/RussetProbe", &["fat Mach-O magic"]));
        assert!(release.admits("RussetProbe.app.dSYM/Contents/Resources/DWARF/RussetProbe", &["Mach-O magic"]));
        assert!(!release.admits("RussetProbe.app/Frameworks/Foo.framework/Foo", &["Mach-O magic"]));
        assert!(!release.admits("RussetProbe.app/RussetProbe", &["Mach-O magic", "Apple copyright header"]));
        for (workflow, job) in [("ci.yml", "release"), ("apple-release.yml", "build-other"), ("nightly.yml", "full")] {
            let other = UploadAllow::load(&file, workflow, job).expect("parse");
            assert!(!other.admits("RussetProbe.app/RussetProbe", &["Mach-O magic"]), "{workflow}/{job}");
        }
    }

    /// The committed synthetic allowlist parses, and every entry names its owner.
    #[test]
    fn the_committed_synthetic_allowlist_is_well_formed() {
        let path = crate::workstreams::repo_root().join(SYNTHETIC_ALLOWLIST);
        let text = fs::read_to_string(&path).expect("read");
        SyntheticAllowlist::parse(&text).expect("parse");
        let raw: SyntheticFile = toml::from_str(&text).expect("toml");
        for e in &raw.synthetic {
            let owners_ok = e.owner.split(", ").all(|o| {
                o == "lead"
                    || o.strip_prefix('W')
                        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric()))
            });
            assert!(owners_ok, "owner {:?} of {}", e.owner, e.sha256);
            assert_eq!(e.sha256, e.sha256.to_lowercase(), "lowercase hex");
        }
    }

    /// The clean tree passes the built-in guards (QA-02 acceptance: "the clean tree passes").
    #[test]
    fn the_repository_passes_the_built_in_guards() {
        let root = crate::workstreams::repo_root();
        let checks = [Check::AppleFiles, Check::Secrets, Check::PublicTerms];
        let s = run_checks(&root, &checks, &builtin(Vec::new())).expect("guard");
        assert!(s.failed.is_empty(), "{:#?}", s.findings);
    }
}
