// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Project detection (build §9.1; `project.detect`, `project.info`).
//!
//! The project root is the nearest directory, from the one given upwards, that holds `russet.toml` or a stack
//! marker; none at all is `config.projectNotFound` (1301). The stack is `russet.toml [project] stack` when set,
//! else the one marker in the root, in this order: `pubspec.yaml` with `flutter:` (flutter), `*.xcworkspace`,
//! `*.xcodeproj` or `*.xcproj` (xcode), `package.json` naming `react-native` (react-native), a `*.csproj` targeting
//! `net*-ios` (maui), `Package.swift` (swiftpm). Two markers are `build.projectAmbiguous` (4000) with the
//! candidates.

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::manifest::{LOCAL_FILE, PROJECT_FILE, Stack};

/// The stack markers found in one directory, with the files that showed them.
pub fn markers(dir: &Utf8Path) -> Vec<(Stack, Utf8PathBuf)> {
    let mut found = Vec::new();
    let Ok(entries) = dir.read_dir_utf8() else { return found };
    let mut names: Vec<String> = entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_owned()).collect();
    names.sort();
    let contains = |name: &str, needle: &str| std::fs::read_to_string(dir.join(name)).is_ok_and(|t| t.contains(needle));
    for name in &names {
        let stack = if name == "pubspec.yaml" && contains(name, "flutter:") {
            Some(Stack::Flutter)
        } else if name.ends_with(".xcworkspace") || name.ends_with(".xcodeproj") || name.ends_with(".xcproj") {
            Some(Stack::Xcode)
        } else if name == "package.json" && contains(name, "\"react-native\"") {
            Some(Stack::ReactNative)
        } else if name.ends_with(".csproj") && contains(name, "-ios") {
            Some(Stack::Maui)
        } else if name == "Package.swift" {
            Some(Stack::Swiftpm)
        } else {
            None
        };
        if let Some(s) = stack
            && !found.iter().any(|(f, _)| *f == s)
        {
            found.push((s, dir.join(name)));
        }
    }
    let order = [Stack::Flutter, Stack::Xcode, Stack::ReactNative, Stack::Maui, Stack::Swiftpm];
    found.sort_by_key(|(s, _)| order.iter().position(|o| o == s));
    found
}

/// The project root for `start`: the nearest ancestor (or `start` itself) with `russet.toml` or a stack marker.
pub fn find_root(start: &Utf8Path) -> Result<Utf8PathBuf> {
    let start = if start.is_relative() {
        let cwd = std::env::current_dir().ok().and_then(|p| Utf8PathBuf::from_path_buf(p).ok()).unwrap_or_default();
        cwd.join(start)
    } else {
        start.to_owned()
    };
    for dir in start.ancestors() {
        if dir.join(PROJECT_FILE).is_file() || dir.join(LOCAL_FILE).is_file() || !markers(dir).is_empty() {
            return Ok(dir.to_owned());
        }
    }
    Err(Error::new(codes::config::PROJECT_NOT_FOUND, format!("no project at {start} or above it"))
        .with_hint(
            "Run Russet in an app project: a directory with russet.toml, or a SwiftPM package with Package.swift.",
        )
        .with_details(serde_json::json!({"dir": start})))
}

/// The stack of the project at `root`, and the marker files.
pub fn stack(root: &Utf8Path, declared: Option<Stack>) -> Result<(Stack, Vec<Utf8PathBuf>)> {
    let found = markers(root);
    let files: Vec<Utf8PathBuf> = found.iter().map(|(_, f)| f.clone()).collect();
    if let Some(s) = declared {
        return Ok((s, files));
    }
    match found.as_slice() {
        [] => Err(Error::new(codes::config::PROJECT_NOT_FOUND, format!("cannot tell which kind of project {root} is"))
            .with_hint("Set `[project] stack` in russet.toml, or add the stack's own manifest (Package.swift for SwiftPM).")),
        [(s, _)] => Ok((*s, files)),
        many => Err(Error::new(
            codes::build::PROJECT_AMBIGUOUS,
            format!("{root} matches several stacks: {}", many.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(", ")),
        )
        .with_hint("Set `[project] stack` in russet.toml.")
        .with_details(serde_json::json!({"candidates": many.iter().map(|(s, f)| serde_json::json!({"stack": s.as_str(), "file": f})).collect::<Vec<_>>()}))),
    }
}

/// Library product names declared in `Package.swift`, read without running SwiftPM (a fast, best-effort scan for
/// `project.detect`; builds use `swift package describe`).
pub fn swiftpm_products(package_swift: &Utf8Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(package_swift) else { return Vec::new() };
    let mut out = Vec::new();
    for (idx, _) in text.match_indices(".library(") {
        let rest = &text[idx..];
        let Some(name_at) = rest.find("name:") else { continue };
        let after = rest[name_at + 5..].trim_start();
        if let Some(stripped) = after.strip_prefix('"')
            && let Some(end) = stripped.find('"')
        {
            let name = stripped[..end].to_owned();
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = Utf8PathBuf::from_path_buf(d.path().canonicalize().unwrap()).unwrap();
        (d, p)
    }

    #[test]
    fn the_root_is_found_upwards() {
        let (_d, root) = dir();
        std::fs::write(
            root.join("Package.swift"),
            "let package = Package(name: \"A\", products: [.library(name: \"A\", targets: [\"A\"])])",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("Sources/A")).unwrap();
        assert_eq!(find_root(&root.join("Sources/A")).unwrap(), root);
        assert_eq!(stack(&root, None).unwrap().0, Stack::Swiftpm);
        assert_eq!(swiftpm_products(&root.join("Package.swift")), vec!["A".to_owned()]);
    }

    #[test]
    fn nothing_is_project_not_found() {
        let (_d, root) = dir();
        let e = find_root(&root).unwrap_err();
        // A temporary directory has no project above it either, unless the machine keeps one in /tmp's parents.
        assert_eq!(e.code(), codes::config::PROJECT_NOT_FOUND);
        assert!(e.hint().is_some());
    }

    #[test]
    fn two_markers_are_ambiguous_unless_declared() {
        let (_d, root) = dir();
        std::fs::write(root.join("Package.swift"), "").unwrap();
        std::fs::create_dir_all(root.join("App.xcodeproj")).unwrap();
        let e = stack(&root, None).unwrap_err();
        assert_eq!(e.code(), codes::build::PROJECT_AMBIGUOUS);
        assert_eq!(stack(&root, Some(Stack::Swiftpm)).unwrap().0, Stack::Swiftpm);
        assert_eq!(markers(&root)[0].0, Stack::Xcode, "detection order puts xcode before swiftpm");
    }

    #[test]
    fn flutter_and_rn_need_their_content() {
        let (_d, root) = dir();
        std::fs::write(root.join("pubspec.yaml"), "name: x\nflutter:\n  uses-material-design: true\n").unwrap();
        std::fs::write(root.join("package.json"), "{\"dependencies\": {\"left-pad\": \"1\"}}").unwrap();
        assert_eq!(stack(&root, None).unwrap().0, Stack::Flutter);
    }
}
