// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask check-layers`: enforces ARCHITECTURE §5.2 from `cargo metadata` and `.config/workstreams.toml`.
//!
//! - Every workspace package, binaries included, has a layer.
//! - F → F only; L0 → no Russet crate; L1 → L0; L2 → L0–L1 plus the listed L2 grants; L3 → F and L0–L2 except
//!   `l3-never`, and L3 → L3 only along the transitive closure of the listed grants; packages under `only` (for
//!   example `russet-adi-host`, `russet-mcp`, `russet-cli`) use only what they list; `russetd` and dev packages
//!   use anything; nothing but dev packages uses a dev package.
//! - Layer F is permissive: its own licence and the licence of every package it depends on (transitively, normal
//!   and build dependencies) must be permissive.
//!
//! Normal and build dependencies count; dev-dependencies do not (tests may use the testkit, ARCHITECTURE §5.1).
//! The graph is resolved with **all features** of every workspace package, so optional dependencies (for
//! example russet-xar's `udif`, `hfsplus` and `cms` readers) are checked too; `deny.toml` does the same.
//! Owner: W1.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use anyhow::{Context, Result};
use cargo_metadata::{CargoOpt, DependencyKind, MetadataCommand};

use crate::workstreams::{self, LayerRules, Workstreams};

/// Flags of `cargo xtask check-layers`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Also print every checked edge.
    #[arg(long)]
    pub verbose: bool,
}

pub fn run(args: Args) -> Result<()> {
    let root = workstreams::repo_root();
    let cfg = workstreams::load(&root)?;
    let metadata = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .features(CargoOpt::AllFeatures)
        .exec()
        .context("running cargo metadata --all-features")?;
    let graph = Graph::from_metadata(&metadata);
    if args.verbose {
        for p in graph.packages.values().filter(|p| p.workspace) {
            for d in &p.deps {
                println!("{} -> {}", p.name, graph.packages[d].name);
            }
        }
    }
    let violations = check(&graph, &cfg);
    let members = graph.packages.values().filter(|p| p.workspace).count();
    if violations.is_empty() {
        println!(
            "check-layers: {members} workspace packages, {} packages in the graph (all features): ok",
            graph.packages.len()
        );
        return Ok(());
    }
    for v in &violations {
        println!("{v}");
    }
    anyhow::bail!("check-layers: {} violation(s)", violations.len())
}

/// One package of the dependency graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub version: String,
    pub workspace: bool,
    pub license: Option<String>,
    /// Ids of normal and build dependencies (dev-dependencies are left out).
    pub deps: Vec<String>,
}

/// The resolved dependency graph, keyed by package id.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub packages: BTreeMap<String, Node>,
}

impl Graph {
    pub fn from_metadata(metadata: &cargo_metadata::Metadata) -> Self {
        let members: BTreeSet<&cargo_metadata::PackageId> = metadata.workspace_members.iter().collect();
        let mut packages: BTreeMap<String, Node> = metadata
            .packages
            .iter()
            .map(|p| {
                let node = Node {
                    name: p.name.to_string(),
                    version: p.version.to_string(),
                    workspace: members.contains(&p.id),
                    license: p.license.clone(),
                    deps: Vec::new(),
                };
                (p.id.repr.clone(), node)
            })
            .collect();
        if let Some(resolve) = &metadata.resolve {
            for node in &resolve.nodes {
                let deps: Vec<String> = node
                    .deps
                    .iter()
                    .filter(|d| d.dep_kinds.iter().any(|k| k.kind != DependencyKind::Development))
                    .map(|d| d.pkg.repr.clone())
                    .collect();
                if let Some(n) = packages.get_mut(&node.id.repr) {
                    n.deps = deps;
                }
            }
        }
        Self { packages }
    }
}

/// A layering or licence violation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Violation {
    NoLayer { package: String },
    Edge { from: String, from_layer: String, to: String, to_layer: String },
    NotPermissive { f_crate: String, package: String, license: String },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLayer { package } => {
                write!(f, "{package}: no layer in .config/workstreams.toml [layers] (ARCHITECTURE §5.1)")
            }
            Self::Edge { from, from_layer, to, to_layer } => {
                write!(f, "{from} ({from_layer}) may not depend on {to} ({to_layer}) (ARCHITECTURE §5.2)")
            }
            Self::NotPermissive { f_crate, package, license } => write!(
                f,
                "{f_crate} is layer F (permissive) but depends on {package}, licensed `{license}` (ARCHITECTURE §5.2)"
            ),
        }
    }
}

/// Every violation in `graph` under `cfg`, sorted.
pub fn check(graph: &Graph, cfg: &Workstreams) -> Vec<Violation> {
    let mut out = BTreeSet::new();
    let l3_closure = closure(&cfg.layer_rules.l3_may_use);
    let layer_of = |name: &str| cfg.layers.get(name).map(String::as_str);
    for p in graph.packages.values().filter(|p| p.workspace) {
        let Some(from_layer) = layer_of(&p.name) else {
            out.insert(Violation::NoLayer { package: p.name.clone() });
            continue;
        };
        for dep_id in &p.deps {
            let Some(dep) = graph.packages.get(dep_id) else { continue };
            if !dep.workspace {
                continue;
            }
            let Some(to_layer) = layer_of(&dep.name) else { continue };
            if !edge_allowed(&p.name, from_layer, &dep.name, to_layer, &cfg.layer_rules, &l3_closure) {
                out.insert(Violation::Edge {
                    from: p.name.clone(),
                    from_layer: from_layer.to_owned(),
                    to: dep.name.clone(),
                    to_layer: to_layer.to_owned(),
                });
            }
        }
        if from_layer == "F" {
            let own = p.license.clone().unwrap_or_default();
            if !is_permissive(&own, &cfg.layer_rules.permissive) {
                out.insert(Violation::NotPermissive {
                    f_crate: p.name.clone(),
                    package: p.name.clone(),
                    license: display_license(&p.license),
                });
            }
            for id in transitive_deps(graph, dep_ids(graph, p)) {
                let dep = &graph.packages[&id];
                let license = dep.license.clone().unwrap_or_default();
                if !is_permissive(&license, &cfg.layer_rules.permissive) {
                    out.insert(Violation::NotPermissive {
                        f_crate: p.name.clone(),
                        package: format!("{} {}", dep.name, dep.version),
                        license: display_license(&dep.license),
                    });
                }
            }
        }
    }
    out.into_iter().collect()
}

fn display_license(l: &Option<String>) -> String {
    l.clone().unwrap_or_else(|| "none (license-file only)".to_owned())
}

fn dep_ids<'a>(graph: &'a Graph, p: &'a Node) -> impl Iterator<Item = String> + 'a {
    p.deps.iter().filter(|id| graph.packages.contains_key(*id)).cloned()
}

fn transitive_deps(graph: &Graph, start: impl Iterator<Item = String>) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<String> = start.collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(n) = graph.packages.get(&id) {
            stack.extend(n.deps.iter().filter(|d| !seen.contains(*d)).cloned());
        }
    }
    seen
}

/// Transitive closure of "dependent may use dependency" grants.
fn closure(grants: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, BTreeSet<String>> {
    let mut out = BTreeMap::new();
    for start in grants.keys() {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<&String> = grants[start].iter().collect();
        while let Some(next) = stack.pop() {
            if seen.insert(next.clone())
                && let Some(more) = grants.get(next)
            {
                stack.extend(more.iter());
            }
        }
        out.insert(start.clone(), seen);
    }
    out
}

fn edge_allowed(
    from: &str,
    from_layer: &str,
    to: &str,
    to_layer: &str,
    rules: &LayerRules,
    l3_closure: &BTreeMap<String, BTreeSet<String>>,
) -> bool {
    if from_layer == "dev" {
        return true;
    }
    if to_layer == "dev" {
        return false;
    }
    if let Some(only) = rules.only.get(from) {
        return only.iter().any(|allowed| allowed == to_layer || allowed == to);
    }
    let granted = |grants: &BTreeMap<String, Vec<String>>| grants.get(from).is_some_and(|g| g.iter().any(|d| d == to));
    match from_layer {
        "F" => to_layer == "F",
        "L0" => false,
        "L1" => to_layer == "L0",
        "L2" => matches!(to_layer, "L0" | "L1") || (to_layer == "L2" && granted(&rules.l2_may_use)),
        "L3" => match to_layer {
            "F" | "L0" | "L1" | "L2" => !rules.l3_never.iter().any(|n| n == to),
            "L3" => l3_closure.get(from).is_some_and(|c| c.contains(to)),
            _ => false,
        },
        // russetd: anything but dev packages.
        "L4" => true,
        _ => false,
    }
}

/// Whether an SPDX expression is permissive: `OR` needs one permissive side, `AND` needs both, `X WITH Y` counts
/// as `X`, and the legacy `/` separator means `OR`.
pub fn is_permissive(expr: &str, permissive: &[String]) -> bool {
    let tokens = tokenize(expr);
    if tokens.is_empty() {
        return false;
    }
    let mut parser = SpdxParser { tokens: &tokens, pos: 0, permissive };
    match parser.or_expr() {
        Some(v) => parser.pos == tokens.len() && v,
        None => false,
    }
}

fn tokenize(expr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in expr.chars() {
        match ch {
            '(' | ')' | '/' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(if ch == '/' { "OR".to_owned() } else { ch.to_string() });
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

struct SpdxParser<'a> {
    tokens: &'a [String],
    pos: usize,
    permissive: &'a [String],
}

impl SpdxParser<'_> {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(String::as_str)
    }

    fn or_expr(&mut self) -> Option<bool> {
        let mut v = self.and_expr()?;
        while self.peek() == Some("OR") {
            self.pos += 1;
            let rhs = self.and_expr()?;
            v = v || rhs;
        }
        Some(v)
    }

    fn and_expr(&mut self) -> Option<bool> {
        let mut v = self.atom()?;
        while self.peek() == Some("AND") {
            self.pos += 1;
            let rhs = self.atom()?;
            v = v && rhs;
        }
        Some(v)
    }

    fn atom(&mut self) -> Option<bool> {
        match self.peek()? {
            "(" => {
                self.pos += 1;
                let v = self.or_expr()?;
                (self.peek() == Some(")")).then_some(())?;
                self.pos += 1;
                Some(v)
            }
            ")" | "AND" | "OR" | "WITH" => None,
            id => {
                let id = id.trim_end_matches('+').to_owned();
                self.pos += 1;
                if self.peek() == Some("WITH") {
                    self.pos += 2;
                }
                Some(self.permissive.iter().any(|p| p.eq_ignore_ascii_case(&id)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permissive() -> Vec<String> {
        ["MIT", "Apache-2.0", "BSD-3-Clause", "ISC", "Unicode-3.0", "Zlib"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn spdx_expressions() {
        let p = permissive();
        assert!(is_permissive("MIT", &p));
        assert!(is_permissive("MIT OR Apache-2.0", &p));
        assert!(is_permissive("MIT/Apache-2.0", &p));
        assert!(is_permissive("(MIT OR Apache-2.0) AND Unicode-3.0", &p));
        assert!(is_permissive("Apache-2.0 WITH LLVM-exception", &p));
        assert!(is_permissive("MIT OR Apache-2.0 OR LGPL-2.1-or-later", &p));
        assert!(!is_permissive("MPL-2.0", &p));
        assert!(!is_permissive("GPL-3.0-or-later", &p));
        assert!(!is_permissive("MIT AND GPL-3.0-or-later", &p));
        assert!(!is_permissive("", &p));
        assert!(!is_permissive("MIT AND", &p));
    }

    fn cfg() -> Workstreams {
        workstreams::parse(
            r#"
schema = 1
[seams]
lead-only = []
[layers]
"russet-plist" = "F"
"russet-ipa" = "F"
"russet-core" = "L0"
"russet-protocol" = "L1"
"russet-ipc" = "L2"
"russet-config" = "L2"
"russet-toolchain" = "L2"
"russet-device" = "L3"
"russet-sign" = "L3"
"russet-build" = "L3"
"russet-account" = "L3"
"russet-asc" = "L3"
"russet-gsa" = "L3"
"russet-anisette" = "L3"
"russet-ship" = "L3"
"russet-adi-host" = "L3"
"russetd" = "L4"
"russet-cli" = "L4"
"russet-testkit" = "dev"
[layer-rules]
permissive = ["MIT", "Apache-2.0"]
l3-never = ["russet-ipc"]
[layer-rules.l2-may-use]
"russet-toolchain" = ["russet-config"]
[layer-rules.l3-may-use]
"russet-build" = []
"russet-sign" = ["russet-build"]
"russet-account" = ["russet-sign", "russet-asc", "russet-gsa"]
"russet-gsa" = ["russet-asc", "russet-anisette"]
"russet-ship" = ["russet-sign", "russet-account", "russet-asc"]
[layer-rules.only]
"russet-adi-host" = ["F", "L0"]
"russet-cli" = ["L0", "L1", "L2", "russetd"]
[ws]
"#,
        )
        .expect("test config parses")
    }

    /// A graph of workspace packages (all MIT unless named in `licenses`) with the given edges.
    fn graph(edges: &[(&str, &str)], licenses: &[(&str, &str)]) -> Graph {
        let mut g = Graph::default();
        let mut add = |name: &str, workspace: bool| {
            g.packages.entry(name.to_owned()).or_insert_with(|| Node {
                name: name.to_owned(),
                version: "0.1.0".to_owned(),
                workspace,
                license: Some(
                    licenses.iter().find(|(n, _)| *n == name).map(|(_, l)| l.to_string()).unwrap_or("MIT".into()),
                ),
                deps: Vec::new(),
            });
        };
        for (a, b) in edges {
            add(a, a.starts_with("russet") || a.starts_with("xtask"));
            add(b, b.starts_with("russet"));
        }
        for (a, b) in edges {
            if let Some(n) = g.packages.get_mut(*a) {
                n.deps.push(b.to_string());
            }
        }
        g
    }

    #[test]
    fn a_legal_graph_passes() {
        let g = graph(
            &[
                ("russet-protocol", "russet-core"),
                ("russet-toolchain", "russet-config"),
                ("russet-device", "russet-ipa"),
                ("russet-device", "russet-core"),
                ("russet-sign", "russet-build"),
                ("russet-ship", "russet-account"),
                ("russet-ship", "russet-anisette"),
                ("russet-adi-host", "russet-plist"),
                ("russet-adi-host", "russet-core"),
                ("russet-cli", "russetd"),
                ("russetd", "russet-device"),
                ("russet-ipa", "zip"),
                ("zip", "flate2"),
            ],
            &[],
        );
        assert_eq!(check(&g, &cfg()), Vec::new());
    }

    #[test]
    fn a_deliberately_illegal_edge_is_rejected() {
        // L0 may depend on no Russet crate; L3 may never use russet-ipc; sign may not use device.
        let g = graph(
            &[("russet-core", "russet-device"), ("russet-device", "russet-ipc"), ("russet-sign", "russet-device")],
            &[],
        );
        let v = check(&g, &cfg());
        let edges: Vec<(&str, &str)> = v
            .iter()
            .filter_map(|v| match v {
                Violation::Edge { from, to, .. } => Some((from.as_str(), to.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            edges,
            vec![("russet-core", "russet-device"), ("russet-device", "russet-ipc"), ("russet-sign", "russet-device")]
        );
    }

    #[test]
    fn restricted_packages_and_dev_crates() {
        let g = graph(
            &[
                ("russet-adi-host", "russet-config"),
                ("russet-cli", "russet-device"),
                ("russet-device", "russet-testkit"),
                ("russet-testkit", "russet-device"),
            ],
            &[],
        );
        let v = check(&g, &cfg());
        assert_eq!(v.len(), 3, "{v:?}");
    }

    #[test]
    fn layer_f_must_stay_permissive() {
        let g = graph(&[("russet-ipa", "zip"), ("zip", "copyleft-thing")], &[("copyleft-thing", "GPL-3.0-only")]);
        let v = check(&g, &cfg());
        assert!(matches!(v.as_slice(), [Violation::NotPermissive { f_crate, .. }] if f_crate == "russet-ipa"), "{v:?}");
        let g = graph(&[("russet-ipa", "russet-core")], &[("russet-core", "GPL-3.0-or-later")]);
        let v = check(&g, &cfg());
        assert!(v.iter().any(|v| matches!(v, Violation::Edge { from, .. } if from == "russet-ipa")), "{v:?}");
    }

    #[test]
    fn packages_without_a_layer_are_reported() {
        let g = graph(&[("russet-newcrate", "russet-core")], &[]);
        assert_eq!(check(&g, &cfg()), vec![Violation::NoLayer { package: "russet-newcrate".into() }]);
    }
}
