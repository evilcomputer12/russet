// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The worktree guard for shared cargo target slots (AGENTS.md §6), checked by `cargo xtask check-layers`.
//!
//! Cargo names a workspace crate's artifacts by its path relative to the workspace root and decides freshness
//! by mtime, so in a target slot shared by several worktrees a checkout whose files are older than another
//! worktree's last build would reuse that build. `.cargo/config.toml` sets `RUSSET_BUILD_WORKTREE` to the
//! worktree root for every compilation, and each crate root reads it:
//!
//! ```text
//! const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");
//! ```
//!
//! Cargo records the value a unit was compiled with and recompiles the unit when it differs, and every target
//! that depends on a recompiled library is recompiled too. So the roots that must read it are library and
//! binary roots, build scripts, and every target of a package without a library (its tests depend on nothing
//! that would be recompiled). Owner: W1.

use std::path::Path;

use anyhow::{Context, Result};

/// The variable `.cargo/config.toml` sets.
pub const VAR: &str = "RUSSET_BUILD_WORKTREE";

/// Target kinds that produce a library other targets link against.
const LIBRARY_KINDS: &[&str] = &["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];

/// Root files of workspace targets that do not read the guard, as paths relative to `root`.
pub fn unguarded(metadata: &cargo_metadata::Metadata, root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for package in metadata.workspace_packages() {
        let kind_of = |t: &cargo_metadata::Target| t.kind.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        let has_library =
            package.targets.iter().any(|t| kind_of(t).iter().any(|k| LIBRARY_KINDS.contains(&k.as_str())));
        for target in &package.targets {
            let kinds = kind_of(target);
            let required = !has_library
                || kinds.iter().any(|k| LIBRARY_KINDS.contains(&k.as_str()) || k == "bin" || k == "custom-build");
            if !required {
                continue;
            }
            let path = target.src_path.as_std_path();
            let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            if !reads_guard(&text) {
                let shown = path.strip_prefix(root).unwrap_or(path);
                out.push(shown.display().to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Whether a source file reads the guard on a line that is not a comment.
pub fn reads_guard(text: &str) -> bool {
    let needle = format!("option_env!(\"{VAR}\")");
    text.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with("//") && line.contains(&needle)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_line_is_recognized() {
        assert!(reads_guard("//! docs\n\nconst _: Option<&str> = option_env!(\"RUSSET_BUILD_WORKTREE\");\n"));
        assert!(!reads_guard("// const _: Option<&str> = option_env!(\"RUSSET_BUILD_WORKTREE\");\n"));
        assert!(!reads_guard("const _: Option<&str> = option_env!(\"RUSSET_WORKTREE\");\n"));
        assert!(!reads_guard("fn main() {}\n"));
    }

    #[test]
    fn every_crate_root_of_the_workspace_reads_the_guard() {
        let root = crate::workstreams::repo_root();
        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(root.join("Cargo.toml"))
            .no_deps()
            .exec()
            .expect("cargo metadata");
        assert_eq!(unguarded(&metadata, &root).expect("crate roots are readable"), Vec::<String>::new());
    }

    #[test]
    fn the_cargo_config_sets_the_guard_per_worktree() {
        let config = std::fs::read_to_string(crate::workstreams::repo_root().join(".cargo/config.toml"))
            .expect(".cargo/config.toml");
        let line = config.lines().find(|l| l.starts_with(VAR)).expect("[env] sets RUSSET_BUILD_WORKTREE");
        for part in ["value = \".\"", "relative = true", "force = true"] {
            assert!(line.contains(part), "{line:?} lacks {part}");
        }
    }
}
