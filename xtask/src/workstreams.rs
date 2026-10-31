// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `.config/workstreams.toml`: the machine-readable copy of PLAN §3 (paths per workstream, seam files) and of
//! ARCHITECTURE §5.1–§5.2 (the layer of every workspace package, binaries included, and the layering rules).
//! Shared by `check-layers` and `check-paths`. Owner: W1.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// The whole file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Workstreams {
    pub schema: u32,
    pub seams: Seams,
    /// Generated files: regenerate, never hand-merge (PLAN §0 rule 2).
    #[serde(default)]
    pub generated: Generated,
    /// Package name → layer (`F`, `L0`–`L4`, `dev`).
    pub layers: BTreeMap<String, String>,
    pub layer_rules: LayerRules,
    /// Workstream id (`W1`, `W16b`, …) → its paths.
    pub ws: BTreeMap<String, Workstream>,
}

/// Lead-only files (PLAN §0 rule 2).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Seams {
    pub lead_only: Vec<String>,
}

/// Generated files (PLAN §0 rule 2): every card that changes their inputs regenerates and commits them, so
/// `check-paths` reports them as notes, never as warnings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Generated {
    pub paths: Vec<String>,
}

/// Layering rules beyond the layer table (ARCHITECTURE §5.2).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LayerRules {
    /// The only L2 → L2 grants: dependent → dependencies it may use.
    #[serde(default)]
    pub l2_may_use: BTreeMap<String, Vec<String>>,
    /// Direct L3 → L3 grants (dependent → dependencies); check-layers takes the transitive closure.
    #[serde(default)]
    pub l3_may_use: BTreeMap<String, Vec<String>>,
    /// Lower-layer crates that no L3 crate may use (`russet-ipc`).
    #[serde(default)]
    pub l3_never: Vec<String>,
    /// Packages restricted to an explicit list of layers and package names.
    #[serde(default)]
    pub only: BTreeMap<String, Vec<String>>,
    /// SPDX ids that count as permissive for layer F's dependencies.
    pub permissive: Vec<String>,
}

/// One workstream's paths (repository-relative globs).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Workstream {
    pub title: String,
    pub wave: String,
    #[serde(default)]
    pub lane: String,
    /// The bootstrap card (W1) writes the first version of everything; it is left out of other cards' owner
    /// lists.
    #[serde(default)]
    pub bootstrap: bool,
    #[serde(default)]
    pub owns: Vec<String>,
    #[serde(default)]
    pub except: Vec<String>,
    /// A card of the same wave that must merge before this one starts: a sequential handover (PLAN §2, §3), so
    /// the two may own the same paths. Within a wave, every other pair of cards owns disjoint paths.
    #[serde(default)]
    pub after: Option<String>,
}

/// The repository root, found at run time: the nearest ancestor of the current directory whose `Cargo.toml`
/// declares `[workspace]` and has an `xtask/` member; else the parent of the `CARGO_MANIFEST_DIR` that cargo
/// sets for `cargo run`; else, last, the directory this binary was compiled in.
///
/// Never trust the compiled-in path first: in a cargo target slot shared by several worktrees, `cargo xtask`
/// can run a binary another worktree built, and that binary would then inspect the other tree (AGENTS.md §6).
pub fn repo_root() -> PathBuf {
    let from_cwd = std::env::current_dir().ok().and_then(|cwd| workspace_root_above(&cwd));
    let from_cargo = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .and_then(|dir| dir.parent().and_then(workspace_root_above));
    from_cwd.or(from_cargo).unwrap_or_else(|| {
        let compiled = Path::new(env!("CARGO_MANIFEST_DIR"));
        compiled.parent().unwrap_or(compiled).to_path_buf()
    })
}

/// The nearest directory at or above `start` that holds Russet's workspace manifest.
fn workspace_root_above(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| is_workspace_root(dir)).map(Path::to_path_buf)
}

fn is_workspace_root(dir: &Path) -> bool {
    let declares_workspace = std::fs::read_to_string(dir.join("Cargo.toml"))
        .is_ok_and(|text| text.lines().any(|line| line.trim() == "[workspace]"));
    declares_workspace && dir.join("xtask/Cargo.toml").is_file()
}

/// Loads `<root>/.config/workstreams.toml`.
pub fn load(root: &Path) -> Result<Workstreams> {
    let path = root.join(".config/workstreams.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Parses the file's text.
pub fn parse(text: &str) -> Result<Workstreams> {
    let ws: Workstreams = toml::from_str(text)?;
    anyhow::ensure!(ws.schema == 1, "unsupported workstreams.toml schema {}", ws.schema);
    for (pkg, layer) in &ws.layers {
        anyhow::ensure!(
            matches!(layer.as_str(), "F" | "L0" | "L1" | "L2" | "L3" | "L4" | "dev"),
            "package {pkg} has unknown layer {layer}"
        );
    }
    for (id, card) in &ws.ws {
        if let Some(before) = &card.after {
            let other = ws.ws.get(before).with_context(|| format!("{id}: after = {before:?} is not a card"))?;
            anyhow::ensure!(
                other.wave == card.wave,
                "{id}: after = {before:?} is in wave {}, not {}",
                other.wave,
                card.wave
            );
            anyhow::ensure!(before != id, "{id}: after names itself");
        }
    }
    Ok(ws)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_file_parses() {
        let ws = load(&repo_root()).expect("workstreams.toml parses");
        assert!(ws.ws.contains_key("W1"));
        assert!(ws.layers.contains_key("russet-core"));
        assert!(ws.seams.lead_only.iter().any(|p| p == "xtask/src/main.rs"));
    }

    #[test]
    fn the_root_is_found_from_any_directory_inside_the_worktree() {
        let root = repo_root();
        assert!(is_workspace_root(&root), "{} is not the workspace root", root.display());
        assert_eq!(workspace_root_above(&root.join("crates/russet-core/src")), Some(root.clone()));
        assert_eq!(workspace_root_above(&root.join("xtask")), Some(root));
        // Another checkout with the same layout is its own root, whatever this binary was compiled from.
        let other = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(other.path().join("xtask/src")).expect("mkdir");
        std::fs::write(other.path().join("Cargo.toml"), "[workspace]\nmembers = [\"xtask\"]\n").expect("write");
        std::fs::write(other.path().join("xtask/Cargo.toml"), "[package]\nname = \"xtask\"\n").expect("write");
        assert_eq!(workspace_root_above(&other.path().join("xtask/src")), Some(other.path().to_path_buf()));
        assert_eq!(workspace_root_above(Path::new("/")), None);
    }
}
