// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask check-paths --ws WNN`: warns about edits outside a workstream's paths (PLAN §0 rule 2, §3).
//!
//! The changed files are the branch's diff against the merge base with `--base` (default `main`), plus
//! uncommitted and untracked files; `--path` checks explicit paths instead. Each file is classified as owned
//! (fine), generated (a note: regenerate, never hand-merge; never a warning), a seam file (lead-only after W1
//! merges), owned by another workstream, or unowned. Warnings exit 0 unless `--strict`. Without `--ws`, the
//! workstream comes from the branch name (`ws/w04-device` → `W4`).
//!
//! The unit tests also check `.config/workstreams.toml` itself: within a wave, no two cards own a common path
//! unless one is `after` the other (an exact check over the globs; see `overlap`). Owner: W1.

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
    let mut notes = 0;
    for (path, finding) in &findings {
        match finding {
            Finding::Owned => {}
            Finding::Generated => {
                notes += 1;
                println!("note: {path}: generated: regenerate, never hand-merge (PLAN §0 rule 2)");
            }
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
        "check-paths: {ws} ({}; wave {}, lane {}): {} changed file(s), {warnings} warning(s), {notes} generated",
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
    /// A generated file (`[generated]`): every card regenerates and commits it.
    Generated,
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
    generated: GlobSet,
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
        Ok(Self { seams: set(&cfg.seams.lead_only)?, generated: set(&cfg.generated.paths)?, ws })
    }

    fn owns(&self, ws: &str, path: &str) -> bool {
        self.ws.get(ws).is_some_and(|g| g.owns.is_match(path) && !g.except.is_match(path))
    }

    /// Classifies `path` for workstream `ws`. Generated files are a note for everyone. Seam files are lead-only
    /// for everyone but the bootstrap card, which creates them. Foreign owners in `ws`'s own wave are reported
    /// when there are any, otherwise every owner; the bootstrap card is never reported as an owner.
    pub fn classify(&self, ws: &str, path: &str) -> Finding {
        let me = self.ws.get(ws);
        if self.generated.is_match(path) {
            return Finding::Generated;
        }
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
mod overlap {
    //! The exact within-wave overlap check for `.config/workstreams.toml`.
    //!
    //! A card owns (the union of its `owns` globs) minus (the union of its `except` globs). Each glob becomes an
    //! NFA that follows globset's translation with `literal_separator(true)`: `*` is `[^/]*`, `?` is `[^/]`, a
    //! leading `**/` is `(?:.*/)?`, an inner `/**/` is `/(?:.*/)?`, a trailing `/**` is `/.*`, `**` alone is
    //! `.*`, and `{a,b}` alternates. A breadth-first search over the product of four automata (A's owns and
    //! excepts, B's owns and excepts), on a finite alphabet (every literal character, `/`, and one stand-in for
    //! every other character), decides whether some path belongs to both cards and returns the shortest one.

    use std::collections::{BTreeSet, HashMap, VecDeque};

    #[derive(Debug, Clone, Copy)]
    enum Label {
        Char(char),
        NotSlash,
        Any,
    }

    impl Label {
        fn matches(self, c: char) -> bool {
            match self {
                Label::Char(l) => l == c,
                Label::NotSlash => c != '/',
                Label::Any => true,
            }
        }
    }

    /// A Thompson NFA for a union of globs.
    #[derive(Debug, Default)]
    pub struct Nfa {
        edges: Vec<Vec<(Label, usize)>>,
        eps: Vec<Vec<usize>>,
        accept: BTreeSet<usize>,
    }

    impl Nfa {
        /// The union of `globs`; state 0 is the start.
        pub fn union(globs: &[String]) -> Self {
            let mut nfa = Nfa::default();
            let start = nfa.state();
            for glob in globs {
                for alternative in expand_braces(glob) {
                    let (s, e) = nfa.glob(&alternative);
                    nfa.eps[start].push(s);
                    nfa.accept.insert(e);
                }
            }
            nfa
        }

        /// Whether the union matches `path` (used to compare the automaton with globset).
        pub fn matches(&self, path: &str) -> bool {
            let mut set = self.closure([0]);
            for c in path.chars() {
                set = self.step(&set, c);
            }
            self.accepts(&set)
        }

        fn state(&mut self) -> usize {
            self.edges.push(Vec::new());
            self.eps.push(Vec::new());
            self.edges.len() - 1
        }

        fn edge(&mut self, from: usize, label: Label) -> usize {
            let to = self.state();
            self.edges[from].push((label, to));
            to
        }

        /// A self-looping state reached by an epsilon edge: `[^/]*` or `.*`.
        fn star(&mut self, from: usize, label: Label) -> usize {
            let s = self.state();
            self.eps[from].push(s);
            self.edges[s].push((label, s));
            s
        }

        /// `(?:.*/)?`
        fn optional_dirs(&mut self, from: usize) -> usize {
            let end = self.state();
            self.eps[from].push(end);
            let any = self.star(from, Label::Any);
            self.edges[any].push((Label::Char('/'), end));
            end
        }

        /// Adds one brace-free glob; returns its start and accepting states.
        fn glob(&mut self, glob: &str) -> (usize, usize) {
            let start = self.state();
            let mut cur = start;
            let components: Vec<&str> = glob.split('/').collect();
            let last = components.len() - 1;
            let mut need_separator = false;
            for (i, component) in components.iter().enumerate() {
                if *component == "**" {
                    if last == 0 {
                        cur = self.star(cur, Label::Any);
                    } else if i == 0 {
                        cur = self.optional_dirs(cur);
                    } else if i == last {
                        cur = self.edge(cur, Label::Char('/'));
                        cur = self.star(cur, Label::Any);
                    } else {
                        cur = self.edge(cur, Label::Char('/'));
                        cur = self.optional_dirs(cur);
                    }
                    need_separator = false;
                    continue;
                }
                if need_separator {
                    cur = self.edge(cur, Label::Char('/'));
                }
                for c in component.chars() {
                    cur = match c {
                        '*' => self.star(cur, Label::NotSlash),
                        '?' => self.edge(cur, Label::NotSlash),
                        '[' | ']' | '{' | '}' | '\\' => panic!("unsupported glob syntax in {glob:?}"),
                        c => self.edge(cur, Label::Char(c)),
                    };
                }
                need_separator = true;
            }
            (start, cur)
        }

        fn closure(&self, states: impl IntoIterator<Item = usize>) -> BTreeSet<usize> {
            let mut out = BTreeSet::new();
            let mut stack: Vec<usize> = states.into_iter().collect();
            while let Some(s) = stack.pop() {
                if out.insert(s) {
                    stack.extend(self.eps[s].iter().copied());
                }
            }
            out
        }

        fn step(&self, set: &BTreeSet<usize>, c: char) -> BTreeSet<usize> {
            let next: Vec<usize> = set
                .iter()
                .flat_map(|s| self.edges[*s].iter())
                .filter(|(label, _)| label.matches(c))
                .map(|(_, to)| *to)
                .collect();
            self.closure(next)
        }

        fn accepts(&self, set: &BTreeSet<usize>) -> bool {
            set.iter().any(|s| self.accept.contains(s))
        }
    }

    /// `a/{b,c}/d{e,f}` → `a/b/de`, `a/b/df`, `a/c/de`, `a/c/df` (no nesting, like globset).
    fn expand_braces(glob: &str) -> Vec<String> {
        let Some(open) = glob.find('{') else { return vec![glob.to_owned()] };
        let close = open + glob[open..].find('}').unwrap_or_else(|| panic!("unclosed brace in {glob:?}"));
        let inner = &glob[open + 1..close];
        assert!(!inner.contains('{'), "nested braces in {glob:?}");
        inner
            .split(',')
            .flat_map(|alt| expand_braces(&format!("{}{alt}{}", &glob[..open], &glob[close + 1..])))
            .collect()
    }

    /// The shortest path owned by both cards (owns minus except), if there is one.
    pub fn common_path(
        a_owns: &[String],
        a_except: &[String],
        b_owns: &[String],
        b_except: &[String],
    ) -> Option<String> {
        let globs = [a_owns, a_except, b_owns, b_except];
        let nfas = globs.map(Nfa::union);
        let mut alphabet: BTreeSet<char> = BTreeSet::from(['/']);
        for glob in globs.iter().flat_map(|g| g.iter()) {
            alphabet.extend(glob.chars().filter(|c| !matches!(c, '*' | '?' | '{' | '}' | ',')));
        }
        let stand_in =
            "xyzqXYZQ_~-0123456789abcdefghijklmnoprstuvw".chars().find(|c| !alphabet.contains(c)).unwrap_or('\u{e000}');
        alphabet.insert(stand_in);

        type Key = [BTreeSet<usize>; 4];
        let start: Key = std::array::from_fn(|i| nfas[i].closure([0]));
        let mut nodes: Vec<(Key, Option<(usize, char)>)> = vec![(start.clone(), None)];
        let mut index: HashMap<Key, usize> = HashMap::from([(start, 0)]);
        let mut queue = VecDeque::from([0usize]);
        while let Some(n) = queue.pop_front() {
            let key = nodes[n].0.clone();
            if nfas[0].accepts(&key[0])
                && !nfas[1].accepts(&key[1])
                && nfas[2].accepts(&key[2])
                && !nfas[3].accepts(&key[3])
            {
                let mut path = Vec::new();
                let mut at = n;
                while let Some((parent, c)) = nodes[at].1 {
                    path.push(c);
                    at = parent;
                }
                return Some(path.into_iter().rev().collect());
            }
            if key[0].is_empty() || key[2].is_empty() {
                continue;
            }
            for &c in &alphabet {
                let next: Key = std::array::from_fn(|i| nfas[i].step(&key[i], c));
                if next[0].is_empty() || next[2].is_empty() || index.contains_key(&next) {
                    continue;
                }
                index.insert(next.clone(), nodes.len());
                queue.push_back(nodes.len());
                nodes.push((next, Some((n, c))));
            }
        }
        None
    }
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
[generated]
paths = ["Cargo.lock", "schemas/**"]
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
owns = ["crates/russet-core/**", "schemas/rpc/**"]
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
        // Generated files are notes for everyone, their generator's owner included.
        assert_eq!(o.classify("W4", "Cargo.lock"), Finding::Generated);
        assert_eq!(o.classify("W4", "schemas/rpc/v1/russet-protocol.schema.json"), Finding::Generated);
        assert_eq!(o.classify("W2", "schemas/rpc/v1/russet-protocol.schema.json"), Finding::Generated);
    }

    #[test]
    fn after_must_name_a_card_of_the_same_wave() {
        let cfg = |after: &str, wave: &str| {
            format!(
                "schema = 1\n[seams]\nlead-only = []\n[layers]\n[layer-rules]\npermissive = []\n\
                 [ws.W4]\ntitle = \"a\"\nwave = \"1\"\n[ws.W9]\ntitle = \"b\"\nwave = \"{wave}\"\nafter = \"{after}\"\n"
            )
        };
        assert!(workstreams::parse(&cfg("W4", "1")).is_ok());
        assert!(workstreams::parse(&cfg("W4", "2")).is_err());
        assert!(workstreams::parse(&cfg("W5", "1")).is_err());
        assert!(workstreams::parse(&cfg("W9", "1")).is_err());
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
        // Generated files that every card commits (PLAN §0 rules 2 and 10).
        assert_eq!(o.classify("W5", "Cargo.lock"), Finding::Generated);
        assert_eq!(o.classify("W3", "schemas/rpc/v1/russet-protocol.schema.json"), Finding::Generated);
        assert_eq!(o.classify("W6", "extension/src/generated/protocol.ts"), Finding::Generated);
        assert_eq!(o.classify("W4", "docs/reference/rpc.md"), Finding::Generated);
        // W4 → W9 hand over the device queue inside Wave 1.
        assert_eq!(o.classify("W4", "docs/lab/queue.md"), Finding::Owned);
        assert_eq!(o.classify("W9", "docs/lab/queue.md"), Finding::Owned);
    }

    #[test]
    fn the_committed_map_matches_the_later_cards() {
        let cfg = workstreams::load(&workstreams::repo_root()).expect("workstreams.toml parses");
        let o = Owners::new(&cfg).expect("globs compile");
        // W16 owns russet-tool in Wave 2, phase-A actool and `ibtool --version` included (PLAN §5.7).
        assert_eq!(o.classify("W16", "crates/russet-tool/src/actool.rs"), Finding::Owned);
        assert_eq!(o.classify("W16", "crates/russet-tool/src/ibtool.rs"), Finding::Owned);
        // W12 owns the ship.archive and ship.ipa rows (PLAN §5.3).
        assert_eq!(o.classify("W12", "crates/russet-protocol/src/ship.rs"), Finding::Owned);
        assert_eq!(o.classify("W12", "crates/russetd/src/handlers/ship.rs"), Finding::Owned);
        // W28's pre-created xcode and xctest namespaces (PLAN §9.2).
        for path in [
            "crates/russet-protocol/src/xcode.rs",
            "crates/russet-protocol/src/xctest.rs",
            "crates/russetd/src/handlers/xcode.rs",
            "crates/russetd/src/handlers/xctest.rs",
        ] {
            assert_eq!(o.classify("W28", path), Finding::Owned, "{path}");
        }
        // The within-wave overlaps found in review are excepted.
        assert_eq!(
            o.classify("W17", "crates/russet-cli/tests/flutter_run_hot.rs"),
            Finding::Foreign(vec!["W18".into()])
        );
        assert_eq!(o.classify("W17", "crates/russet-cli/tests/flutter_build_ios.rs"), Finding::Owned);
        assert_eq!(
            o.classify("W27a", "crates/russet-build/src/platform/windows/windows_paths.rs"),
            Finding::Foreign(vec!["W27b".into()])
        );
        assert_eq!(o.classify("W27a", "crates/russet-ipc/src/windows_pipe.rs"), Finding::Owned);
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_overlap_automaton_agrees_with_globset() {
        let globs = [
            "**",
            "**/foo",
            "foo/**",
            "a/**/b",
            "a/*/b",
            "*.rs",
            "crates/**/src/**/windows*.rs",
            "crates/russet-{config,secrets}/**",
            "crates/russet-cli/tests/{daemon,audit}_*.rs",
            "docs/lab/queue.md",
            "x?z",
        ];
        let paths = [
            "",
            "foo",
            "foo/",
            "foo/bar",
            "x/foo",
            "/foo",
            "a/b",
            "a/x/b",
            "a/x/y/b",
            "a//b",
            "ab",
            "lib.rs",
            "src/lib.rs",
            "crates/x/src/windows.rs",
            "crates/x/src/y/z/windows_pipe.rs",
            "crates/x/src/windows/mod.rs",
            "crates/russet-config/src/lib.rs",
            "crates/russet-secrets",
            "crates/russet-secrets/",
            "crates/russet-cli/tests/audit_tail.rs",
            "crates/russet-cli/tests/audit/x.rs",
            "docs/lab/queue.md",
            "docs/lab/queue.mdx",
            "xyz",
            "x/z",
        ];
        for glob in globs {
            let nfa = overlap::Nfa::union(&[glob.to_owned()]);
            let set = super::set(&[glob.to_owned()]).expect("glob compiles");
            for path in paths {
                assert_eq!(nfa.matches(path), set.is_match(path), "glob {glob:?} on {path:?}");
            }
        }
    }

    #[test]
    fn the_overlap_check_finds_the_overlaps_found_in_review_and_their_fixes() {
        let w17 = strings(&["crates/russet-cli/tests/flutter_*.rs"]);
        let w18 = strings(&["crates/russet-cli/tests/flutter_run_*.rs"]);
        let found = overlap::common_path(&w17, &[], &w18, &[]).expect("W17 and W18 overlapped");
        assert!(found.starts_with("crates/russet-cli/tests/flutter_run_"), "{found}");
        assert_eq!(overlap::common_path(&w17, &w18, &w18, &[]), None);

        let w27a = strings(&["crates/**/src/**/windows*.rs", "docs/windows/**"]);
        let w27b = strings(&["crates/russet-build/src/platform/windows/**", "crates/russet-sdk/src/import/windows/**"]);
        let found = overlap::common_path(&w27a, &[], &w27b, &[]).expect("W27a and W27b overlapped");
        assert!(set(&w27a).expect("globs").is_match(&found) && set(&w27b).expect("globs").is_match(&found), "{found}");
        assert_eq!(overlap::common_path(&w27a, &w27b, &w27b, &[]), None);

        let w4 = strings(&["crates/russet-device/**", "docs/lab/queue.md"]);
        let w9 = strings(&["tests/lab/**", "docs/lab/**"]);
        assert_eq!(overlap::common_path(&w4, &[], &w9, &[]).as_deref(), Some("docs/lab/queue.md"));
        assert_eq!(overlap::common_path(&w4, &[], &strings(&["crates/russet-devices/**"]), &[]), None);
    }

    /// PLAN §3: within a wave, owned paths are disjoint, except between a card and the card it is `after`.
    #[test]
    fn cards_of_one_wave_own_disjoint_paths() {
        let cfg = workstreams::load(&workstreams::repo_root()).expect("workstreams.toml parses");
        let owners = Owners::new(&cfg).expect("globs compile");
        let cards: Vec<(&String, &workstreams::Workstream)> = cfg.ws.iter().filter(|(_, w)| !w.bootstrap).collect();
        let mut overlaps = Vec::new();
        for (i, (a, wa)) in cards.iter().enumerate() {
            for (b, wb) in &cards[i + 1..] {
                let handover = wa.after.as_deref() == Some(b.as_str()) || wb.after.as_deref() == Some(a.as_str());
                if wa.wave != wb.wave || handover {
                    continue;
                }
                if let Some(path) = overlap::common_path(&wa.owns, &wa.except, &wb.owns, &wb.except) {
                    assert!(
                        owners.owns(a, &path) && owners.owns(b, &path),
                        "the overlap automaton and globset disagree on {path:?} ({a}, {b})"
                    );
                    overlaps.push(format!("wave {}: {a} and {b} both own {path}", wa.wave));
                }
            }
        }
        assert!(overlaps.is_empty(), "{}", overlaps.join("\n"));
    }
}
