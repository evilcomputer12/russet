// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask check-paths --ws WNN`: warns about edits outside a workstream's paths (PLAN §0 rule 2, §3).
//!
//! The changed files are the branch's diff against the merge base with `--base` (default `main`), plus
//! uncommitted and untracked files; `--path` checks explicit paths instead. Each file is classified as owned
//! (fine), a seam file (lead-only after W1 merges), owned by another workstream, or unowned. Warnings exit 0
//! unless `--strict`. Without `--ws`, the workstream comes from the branch name (`ws/w04-device` → `W4`).
//! Owner: W1.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

use crate::workstreams::{self, Workstreams};

/// Flags of `cargo xtask check-paths`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The workstream, for example W4 or W16b (default: from the branch name ws/wNN-…)
    #[arg(long)]
    pub ws: Option<String>,
    /// The branch to diff against
    #[arg(long, default_value = "main")]
    pub base: String,
    /// Check these repository-relative paths instead of the git diff
    #[arg(long = "path", value_name = "PATH")]
    pub paths: Vec<String>,
    /// Exit non-zero when anything is reported
    #[arg(long)]
    pub strict: bool,
}

pub fn run(args: Args) -> Result<()> {
    let root = workstreams::repo_root();
    let cfg = workstreams::load(&root)?;
    let ws = match &args.ws {
        Some(ws) => normalize_ws(ws).with_context(|| format!("--ws {ws}"))?,
        None => {
            let branch = git(&root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
            normalize_ws(branch.trim())
                .with_context(|| format!("no --ws given and branch {branch:?} is not ws/wNN-…"))?
        }
    };
    if !cfg.ws.contains_key(&ws) {
        bail!("{ws} is not in .config/workstreams.toml");
    }
    let changed = if args.paths.is_empty() { changed_files(&root, &args.base)? } else { args.paths.clone() };
    let owners = Owners::new(&cfg)?;
    let findings: Vec<(String, Finding)> = changed.iter().map(|p| (p.clone(), owners.classify(&ws, p))).collect();
    let mut warnings = 0;
    for (path, finding) in &findings {
        match finding {
            Finding::Owned => {}
            Finding::Seam => {
                warnings += 1;
                println!("warning: {path}: seam file, lead-only after W1 merges (PLAN §0 rule 2)");
            }
            Finding::Foreign(others) => {
                warnings += 1;
                println!("warning: {path}: owned by {}, not {ws}", others.join(", "));
            }
            Finding::Unowned => {
                warnings += 1;
                println!("warning: {path}: owned by no workstream in .config/workstreams.toml");
            }
        }
    }
    let card = &cfg.ws[&ws];
    println!(
        "check-paths: {ws} ({}; wave {}, lane {}): {} changed file(s), {warnings} warning(s)",
        card.title,
        card.wave,
        card.lane,
        findings.len()
    );
    if args.strict && warnings > 0 {
        bail!("check-paths --strict: {warnings} warning(s)");
    }
    Ok(())
}

/// How a changed path relates to a workstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    Owned,
    Seam,
    Foreign(Vec<String>),
    Unowned,
}

struct WsGlobs {
    owns: GlobSet,
    except: GlobSet,
    wave: String,
    bootstrap: bool,
}

/// Compiled ownership globs.
pub struct Owners {
    seams: GlobSet,
    ws: BTreeMap<String, WsGlobs>,
}

fn glob(pattern: &str) -> Result<Glob> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .with_context(|| format!("bad glob {pattern:?} in workstreams.toml"))
}

fn set(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(glob(p)?);
    }
    Ok(b.build()?)
}

impl Owners {
    pub fn new(cfg: &Workstreams) -> Result<Self> {
        let mut ws = BTreeMap::new();
        for (id, w) in &cfg.ws {
            let globs =
                WsGlobs { owns: set(&w.owns)?, except: set(&w.except)?, wave: w.wave.clone(), bootstrap: w.bootstrap };
            ws.insert(id.clone(), globs);
        }
        Ok(Self { seams: set(&cfg.seams.lead_only)?, ws })
    }

    fn owns(&self, ws: &str, path: &str) -> bool {
        self.ws.get(ws).is_some_and(|g| g.owns.is_match(path) && !g.except.is_match(path))
    }

    /// Classifies `path` for workstream `ws`. Seam files are lead-only for everyone but the bootstrap card, which
    /// creates them. Foreign owners in `ws`'s own wave are reported when there are any, otherwise every owner; the
    /// bootstrap card is never reported as an owner.
    pub fn classify(&self, ws: &str, path: &str) -> Finding {
        let me = self.ws.get(ws);
        if !me.is_some_and(|w| w.bootstrap) && self.seams.is_match(path) {
            return Finding::Seam;
        }
        if self.owns(ws, path) {
            return Finding::Owned;
        }
        let others: Vec<&String> =
            self.ws.iter().filter(|(id, g)| !g.bootstrap && self.owns(id, path)).map(|(id, _)| id).collect();
        let same_wave: Vec<String> = others
            .iter()
            .filter(|id| me.is_some_and(|m| self.ws[id.as_str()].wave == m.wave))
            .map(|id| id.to_string())
            .collect();
        if !same_wave.is_empty() {
            return Finding::Foreign(same_wave);
        }
        if others.is_empty() { Finding::Unowned } else { Finding::Foreign(others.into_iter().cloned().collect()) }
    }
}

/// `W4`, `w04`, `4`, `W16B`, `ws/w16b-apple-containers` → `W4` / `W16b`.
pub fn normalize_ws(s: &str) -> Result<String> {
    let s = s.strip_prefix("ws/").unwrap_or(s);
    let s = s.split('-').next().unwrap_or(s);
    let s = s.strip_prefix(['w', 'W']).unwrap_or(s);
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    let suffix: String = s[digits.len()..].to_ascii_lowercase();
    let number: u32 = digits.parse().with_context(|| format!("{s:?} has no workstream number"))?;
    if !suffix.chars().all(|c| c.is_ascii_lowercase()) || suffix.len() > 1 {
        bail!("{s:?} is not a workstream id");
    }
    Ok(format!("W{number}{suffix}"))
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().context("running git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Files changed on this branch since its merge base with `base`, plus uncommitted and untracked files.
fn changed_files(root: &Path, base: &str) -> Result<Vec<String>> {
    let merge_base = git(root, &["merge-base", base, "HEAD"])?;
    let mut files: Vec<String> = git(root, &["diff", "--name-only", merge_base.trim()])?
        .lines()
        .chain(git(root, &["ls-files", "--others", "--exclude-standard"])?.lines())
        .map(str::to_owned)
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workstream_ids_normalize() {
        assert_eq!(normalize_ws("W4").ok().as_deref(), Some("W4"));
        assert_eq!(normalize_ws("w04").ok().as_deref(), Some("W4"));
        assert_eq!(normalize_ws("16B").ok().as_deref(), Some("W16b"));
        assert_eq!(normalize_ws("ws/w16b-apple-containers").ok().as_deref(), Some("W16b"));
        assert_eq!(normalize_ws("ws/w01-bootstrap").ok().as_deref(), Some("W1"));
        assert!(normalize_ws("main").is_err());
        assert!(normalize_ws("Wx").is_err());
    }

    fn owners() -> Owners {
        let cfg = workstreams::parse(
            r#"
schema = 1
[seams]
lead-only = ["Cargo.toml", "crates/russet-core/src/error/codes/mod.rs"]
[layers]
[layer-rules]
permissive = []
[ws.W1]
title = "Bootstrap"
wave = "1"
bootstrap = true
owns = ["Cargo.toml", "crates/*/Cargo.toml"]
[ws.W2]
title = "Core runtime"
wave = "1"
owns = ["crates/russet-core/**"]
except = ["crates/russet-core/src/error/codes/{config,device}.rs"]
[ws.W4]
title = "Device plane I"
wave = "1"
owns = ["crates/russet-device/**", "crates/russet-core/src/error/codes/device.rs"]
"#,
        )
        .expect("test config parses");
        Owners::new(&cfg).expect("globs compile")
    }

    #[test]
    fn paths_are_classified() {
        let o = owners();
        assert_eq!(o.classify("W2", "crates/russet-core/src/dirs.rs"), Finding::Owned);
        assert_eq!(
            o.classify("W2", "crates/russet-core/src/error/codes/device.rs"),
            Finding::Foreign(vec!["W4".into()])
        );
        assert_eq!(o.classify("W4", "crates/russet-core/src/error/codes/device.rs"), Finding::Owned);
        assert_eq!(o.classify("W2", "crates/russet-core/src/error/codes/mod.rs"), Finding::Seam);
        assert_eq!(o.classify("W2", "Cargo.toml"), Finding::Seam);
        assert_eq!(o.classify("W1", "Cargo.toml"), Finding::Owned);
        assert_eq!(o.classify("W2", "crates/russet-core/src/error/codes/config.rs"), Finding::Unowned);
        // `*` never crosses a directory separator.
        assert_eq!(o.classify("W1", "crates/russet-sign/src/Cargo.toml"), Finding::Unowned);
    }

    #[test]
    fn the_committed_map_gives_every_wave1_card_its_paths() {
        let cfg = workstreams::load(&workstreams::repo_root()).expect("workstreams.toml parses");
        let o = Owners::new(&cfg).expect("globs compile");
        assert_eq!(o.classify("W4", "crates/russet-device/src/manager.rs"), Finding::Owned);
        assert_eq!(
            o.classify("W2", "crates/russet-core/src/error/codes/device.rs"),
            Finding::Foreign(vec!["W4".into()])
        );
        assert_eq!(o.classify("W3", "crates/russet-core/src/error/codes/config.rs"), Finding::Owned);
        assert_eq!(o.classify("W7", "crates/russet-testkit/src/debug/mod.rs"), Finding::Owned);
        assert_eq!(
            o.classify("W5", "crates/russet-build/src/stack/flutter/mod.rs"),
            Finding::Foreign(vec!["W17".into()])
        );
        assert_eq!(o.classify("W2", "crates/russet-device/src/manager.rs"), Finding::Foreign(vec!["W4".into()]));
        assert_eq!(o.classify("W6", "crates/russet-protocol/src/lib.rs"), Finding::Seam);
        assert_eq!(o.classify("W2", "xtask/src/main.rs"), Finding::Seam);
        assert_eq!(o.classify("W8", "xtask/src/guard.rs"), Finding::Owned);
        assert_eq!(o.classify("W1", "AGENTS.md"), Finding::Owned);
    }
}
