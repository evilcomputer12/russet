// SPDX-License-Identifier: GPL-3.0-or-later
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
}

/// The repository root (xtask lives in `<root>/xtask`).
pub fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap_or(manifest_dir).to_path_buf()
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
    Ok(ws)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_committed_file_parses() {
        let ws = super::load(&super::repo_root()).expect("workstreams.toml parses");
        assert!(ws.ws.contains_key("W1"));
        assert!(ws.layers.contains_key("russet-core"));
        assert!(ws.seams.lead_only.iter().any(|p| p == "xtask/src/main.rs"));
    }
}
