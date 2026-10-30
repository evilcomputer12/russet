// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Project scaffolding for `project.init` (PLAN W16; ARCHITECTURE §8.3).
//!
//! Writes a new app project from a bundled 0BSD template (`templates/<stack>/`), substituting the app name,
//! display name and bundle id into both file contents and paths. Only the SwiftPM app template exists today;
//! other stacks return [`codes::build::SCAFFOLD_STACK_UNSUPPORTED`] so the surface is honest.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::manifest::Stack;

const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

/// One file the scaffolder writes: its path under the project root and the template body. `{{NAME}}`,
/// `{{DISPLAY_NAME}}` and `{{BUNDLE_ID}}` are substituted in both.
struct TemplateFile {
    path: &'static str,
    contents: &'static str,
}

/// The SwiftPM SwiftUI app template (0BSD). The sources live under `Sources/{{NAME}}/` to match what
/// `project.detect` and the builder expect of a one-library-product SwiftPM app.
const SWIFTPM_APP: &[TemplateFile] = &[
    TemplateFile {
        path: "russet.toml",
        contents: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../templates/swiftpm-app/russet.toml.tmpl")),
    },
    TemplateFile {
        path: "Package.swift",
        contents: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../templates/swiftpm-app/Package.swift.tmpl")),
    },
    TemplateFile {
        path: ".gitignore",
        contents: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../templates/swiftpm-app/gitignore.tmpl")),
    },
    TemplateFile {
        path: "Sources/{{NAME}}/{{NAME}}App.swift",
        contents: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../templates/swiftpm-app/App.swift.tmpl")),
    },
    TemplateFile {
        path: "Sources/{{NAME}}/ContentView.swift",
        contents: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../templates/swiftpm-app/ContentView.swift.tmpl"
        )),
    },
];

/// What to create.
#[derive(Debug, Clone)]
pub struct ScaffoldRequest {
    /// The project root to write into (created if missing).
    pub dir: Utf8PathBuf,
    /// Which template to use.
    pub stack: Stack,
    /// The app's human-readable name (e.g. `My Great App`).
    pub name: String,
    /// The bundle identifier; defaulted from the name when `None`.
    pub bundle_id: Option<String>,
    /// Write even if the directory already has files.
    pub force: bool,
}

/// What was created.
#[derive(Debug, Clone)]
pub struct ScaffoldOutput {
    /// The absolute project root.
    pub root: Utf8PathBuf,
    /// Files written, relative to the root, in sorted order.
    pub files: Vec<String>,
    /// The Swift package / module name derived from the app name.
    pub package: String,
    /// The bundle identifier used.
    pub bundle_id: String,
}

/// Turns a display name into a Swift package/module identifier: keep alphanumerics, capitalise after a word
/// break, drop everything else. `My great-app` -> `MyGreatApp`. A leading digit is prefixed with `App`.
fn package_name(display: &str) -> Result<String> {
    let mut out = String::new();
    let mut upper_next = true;
    for ch in display.chars() {
        if ch.is_ascii_alphanumeric() {
            if upper_next {
                out.extend(ch.to_uppercase());
            } else {
                out.push(ch);
            }
            upper_next = false;
        } else {
            upper_next = true;
        }
    }
    if out.is_empty() {
        return Err(Error::new(
            codes::build::SCAFFOLD_NAME_INVALID,
            format!("the name {display:?} has no letters or digits"),
        )
        .with_hint("Choose a name with at least one letter or digit, e.g. \"My App\"."));
    }
    if out.as_bytes()[0].is_ascii_digit() {
        out.insert_str(0, "App");
    }
    Ok(out)
}

/// A conservative reverse-DNS bundle id from a package name: `com.example.mygreatapp`.
fn default_bundle_id(package: &str) -> String {
    format!("com.example.{}", package.to_ascii_lowercase())
}

/// Validates a user-supplied bundle id: reverse-DNS, `A-Za-z0-9-` labels separated by dots, at least one dot.
fn check_bundle_id(id: &str) -> Result<()> {
    let labels: Vec<&str> = id.split('.').collect();
    let ok = labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        });
    if ok {
        Ok(())
    } else {
        Err(Error::new(codes::build::BUNDLE_ID_INVALID, format!("the bundle identifier {id:?} is not valid"))
            .with_hint("Use reverse-DNS form, e.g. com.example.myapp (letters, digits and hyphens per label)."))
    }
}

fn substitute(s: &str, package: &str, display: &str, bundle_id: &str) -> String {
    s.replace("{{NAME}}", package).replace("{{DISPLAY_NAME}}", display).replace("{{BUNDLE_ID}}", bundle_id)
}

fn is_non_empty_dir(dir: &Utf8Path) -> bool {
    fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(false)
}

/// Creates a new project. Returns the files written. Does not run SwiftPM or touch the network.
pub fn scaffold(req: ScaffoldRequest) -> Result<ScaffoldOutput> {
    let files = match req.stack {
        Stack::Swiftpm => SWIFTPM_APP,
        other => {
            return Err(Error::new(
                codes::build::SCAFFOLD_STACK_UNSUPPORTED,
                format!("no project template exists for the {} stack yet", other.as_str()),
            )
            .with_hint("The `swiftpm` stack is supported today; Flutter and others are coming."));
        }
    };

    let package = package_name(&req.name)?;
    let bundle_id = match req.bundle_id {
        Some(id) => {
            check_bundle_id(&id)?;
            id
        }
        None => default_bundle_id(&package),
    };

    if req.dir.exists() && !req.force && is_non_empty_dir(&req.dir) {
        return Err(Error::new(
            codes::build::SCAFFOLD_TARGET_NOT_EMPTY,
            format!("the directory {} already has files", req.dir),
        )
        .with_hint("Choose an empty directory or pass --force to write into it anyway."));
    }

    fs::create_dir_all(&req.dir)
        .map_err(|e| Error::new(codes::build::SCAFFOLD_TARGET_NOT_EMPTY, format!("cannot create {}: {e}", req.dir)))?;

    let mut written = Vec::with_capacity(files.len());
    for f in files {
        let rel = substitute(f.path, &package, &req.name, &bundle_id);
        let out = req.dir.join(&rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot create {parent}: {e}")))?;
        }
        let body = substitute(f.contents, &package, &req.name, &bundle_id);
        fs::write(&out, body).map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot write {out}: {e}")))?;
        written.push(rel);
    }
    written.sort();

    Ok(ScaffoldOutput { root: req.dir, files: written, package, bundle_id })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn tmp() -> Utf8PathBuf {
        let base = std::env::temp_dir();
        let unique = format!(
            "russet-scaffold-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let p = base.join(unique);
        Utf8PathBuf::from_path_buf(p).expect("utf8 temp path")
    }
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    #[test]
    fn package_name_is_a_swift_identifier() {
        assert_eq!(package_name("My great-app").unwrap(), "MyGreatApp");
        assert_eq!(package_name("hello").unwrap(), "Hello");
        assert_eq!(package_name("2048 game").unwrap(), "App2048Game");
        assert!(package_name("   ").is_err());
    }

    #[test]
    fn default_bundle_id_is_reverse_dns() {
        assert_eq!(default_bundle_id("MyApp"), "com.example.myapp");
        assert!(check_bundle_id("com.example.myapp").is_ok());
        assert!(check_bundle_id("nodot").is_err());
        assert!(check_bundle_id("com..x").is_err());
    }

    #[test]
    fn scaffolds_a_swiftpm_app_that_detects() {
        let dir = tmp();
        let out = scaffold(ScaffoldRequest {
            dir: dir.clone(),
            stack: Stack::Swiftpm,
            name: "My App".into(),
            bundle_id: None,
            force: false,
        })
        .unwrap();
        assert_eq!(out.package, "MyApp");
        assert_eq!(out.bundle_id, "com.example.myapp");
        assert!(out.files.contains(&"russet.toml".to_owned()));
        assert!(out.files.contains(&"Sources/MyApp/MyAppApp.swift".to_owned()));
        let toml = fs::read_to_string(dir.join("russet.toml")).unwrap();
        assert!(toml.contains("name = \"MyApp\""), "{toml}");
        assert!(toml.contains("bundle-id = \"com.example.myapp\""), "{toml}");
        assert!(toml.contains("display-name = \"My App\""), "{toml}");
        let app = fs::read_to_string(dir.join("Sources/MyApp/MyAppApp.swift")).unwrap();
        assert!(app.contains("struct MyAppApp: App"), "{app}");
        assert!(!app.contains("{{"), "no placeholder should remain: {app}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn refuses_a_non_empty_dir_without_force() {
        let dir = tmp();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("existing.txt"), "x").unwrap();
        let err = scaffold(ScaffoldRequest {
            dir: dir.clone(),
            stack: Stack::Swiftpm,
            name: "App".into(),
            bundle_id: None,
            force: false,
        })
        .unwrap_err();
        assert_eq!(err.code(), codes::build::SCAFFOLD_TARGET_NOT_EMPTY);
        // With force it writes anyway.
        assert!(
            scaffold(ScaffoldRequest {
                dir: dir.clone(),
                stack: Stack::Swiftpm,
                name: "App".into(),
                bundle_id: None,
                force: true,
            })
            .is_ok()
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unsupported_stack_is_reported() {
        let err = scaffold(ScaffoldRequest {
            dir: tmp(),
            stack: Stack::Flutter,
            name: "App".into(),
            bundle_id: None,
            force: false,
        })
        .unwrap_err();
        assert_eq!(err.code(), codes::build::SCAFFOLD_STACK_UNSUPPORTED);
    }
}
