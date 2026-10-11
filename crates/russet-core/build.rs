// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Computes the build id `<version>+g<sha>[-dirty]` (ARCHITECTURE §3.3 item 3) for `russet_core::build_info`.
//!
//! - `RUSSET_BUILD_ID` in the environment wins (CI and packaging set it).
//! - Otherwise the short commit of the source tree's `HEAD`, with `-dirty` when tracked files differ from it.
//! - Without git (a source tarball) the id is `<version>+unknown`.
//!
//! The script reruns when `HEAD` or the branch it names moves, not on every edit: rebuilding russet-core
//! recompiles the whole workspace. Between commits the id therefore stays the same, which is why development
//! clients also compare the daemon's executable identity (ARCHITECTURE §3.3, [D 83]).

// Shared cargo target slots: recompile this build script after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=RUSSET_BUILD_ID");
    println!("cargo:rerun-if-changed=build.rs");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned());
    let id = match std::env::var("RUSSET_BUILD_ID") {
        Ok(id) if !id.trim().is_empty() => id.trim().to_owned(),
        _ => from_git(Path::new(&manifest_dir), &version).unwrap_or_else(|| format!("{version}+unknown")),
    };
    println!("cargo:rustc-env=RUSSET_BUILD_ID={id}");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned());
    println!("cargo:rustc-env=RUSSET_TARGET={target}");
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    Some(text.trim().to_owned())
}

fn from_git(dir: &Path, version: &str) -> Option<String> {
    let sha = git(dir, &["rev-parse", "--short=7", "HEAD"])?;
    // Rerun when HEAD moves: the worktree's own HEAD, the branch it points to, and packed refs.
    for watched in ["HEAD", "packed-refs"] {
        if let Some(p) = git(dir, &["rev-parse", "--git-path", watched]) {
            watch(dir, &p);
        }
    }
    if let Some(branch) = git(dir, &["symbolic-ref", "-q", "HEAD"])
        && let Some(p) = git(dir, &["rev-parse", "--git-path", &branch])
    {
        watch(dir, &p);
    }
    let dirty = git(dir, &["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    Some(format!("{version}+g{sha}{}", if dirty { "-dirty" } else { "" }))
}

fn watch(dir: &Path, path: &str) {
    let p = Path::new(path);
    let p = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
    if p.exists() {
        println!("cargo:rerun-if-changed={}", p.display());
    }
}
