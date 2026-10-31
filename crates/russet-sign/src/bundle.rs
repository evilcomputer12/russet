// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The bundle model (SIGN §8.2): an app and its nested bundles (extensions, App Clips, Watch apps, frameworks)
//! and the Mach-O files each one carries, in signing order (deepest first, the app last).

use std::path::{Path, PathBuf};

use russet_core::Result;

use crate::util::{bundle_invalid, read_plist_dict, slash_path, walk_files};

/// What a bundle is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BundleKind {
    App,
    AppExtension,
    AppClip,
    WatchApp,
    Framework,
    /// Any other bundle with an Info.plist (resource bundles are not walked into).
    Bundle,
}

impl BundleKind {
    /// Whether the bundle is app-like: it carries a main executable that needs a profile and entitlements.
    pub fn needs_profile(self) -> bool {
        matches!(self, Self::App | Self::AppExtension | Self::AppClip | Self::WatchApp)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::AppExtension => "app-extension",
            Self::AppClip => "app-clip",
            Self::WatchApp => "watch-app",
            Self::Framework => "framework",
            Self::Bundle => "bundle",
        }
    }
}

/// One Mach-O file of a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeItem {
    /// The bundle's `CFBundleExecutable`.
    MainExecutable(PathBuf),
    /// A framework's binary (the main executable of a nested `.framework`, listed on the framework).
    Framework(PathBuf),
    /// A loose `.dylib` in `Frameworks/`.
    Dylib(PathBuf),
    /// Any other Mach-O file inside the bundle.
    OtherMachO(PathBuf),
}

impl CodeItem {
    /// The file, relative to the bundle that lists it.
    pub fn path(&self) -> &Path {
        match self {
            Self::MainExecutable(p) | Self::Framework(p) | Self::Dylib(p) | Self::OtherMachO(p) => p,
        }
    }
}

/// An opened bundle and its nested bundles.
#[derive(Debug, Clone)]
pub struct AppBundle {
    /// The bundle directory.
    pub root: PathBuf,
    /// The path relative to the top bundle, `/`-separated; `""` for the top bundle.
    pub rel_path: String,
    pub info: plist::Dictionary,
    pub kind: BundleKind,
    pub nested: Vec<AppBundle>,
    /// Mach-O files of this bundle (relative to [`Self::root`]), nested bundles excluded.
    pub code: Vec<CodeItem>,
}

/// Directories that hold nested code bundles in an iOS app.
const NESTED_DIRS: &[&str] = &["PlugIns", "Extensions", "AppClips", "Watch", "Frameworks"];

impl AppBundle {
    /// Opens the bundle at `path` and walks `PlugIns/`, `Extensions/`, `AppClips/`, `Watch/` and `Frameworks/`.
    pub fn open(path: &Path) -> Result<Self> {
        let kind = match path.extension().and_then(|e| e.to_str()) {
            Some("app") => BundleKind::App,
            Some("appex") => BundleKind::AppExtension,
            Some("framework") => BundleKind::Framework,
            _ => BundleKind::Bundle,
        };
        Self::open_as(path, path, kind)
    }

    fn open_as(top: &Path, path: &Path, kind: BundleKind) -> Result<Self> {
        if !path.is_dir() {
            return Err(bundle_invalid(path, "not a directory"));
        }
        let info_path = path.join("Info.plist");
        if !info_path.is_file() {
            return Err(bundle_invalid(path, "no Info.plist (iOS bundles are shallow)"));
        }
        let info = read_plist_dict(&info_path).map_err(|e| bundle_invalid(path, e.message()))?;
        let rel_path = path.strip_prefix(top).map(slash_path).unwrap_or_default();

        let mut nested = Vec::new();
        for dir in NESTED_DIRS {
            let d = path.join(dir);
            if !d.is_dir() {
                continue;
            }
            let mut children: Vec<_> = std::fs::read_dir(&d)
                .map_err(|e| crate::util::io_error("cannot read", &d, e))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir() && !p.is_symlink())
                .collect();
            children.sort();
            for child in children {
                let ext = child.extension().and_then(|e| e.to_str()).unwrap_or("");
                let child_kind = match (*dir, ext) {
                    (_, "appex") => BundleKind::AppExtension,
                    (_, "framework") => BundleKind::Framework,
                    ("AppClips", "app") => BundleKind::AppClip,
                    ("Watch", "app") => BundleKind::WatchApp,
                    (_, "app") => BundleKind::App,
                    _ => continue,
                };
                if child.join("Info.plist").is_file() {
                    nested.push(Self::open_as(top, &child, child_kind)?);
                }
            }
        }

        let nested_roots: Vec<PathBuf> = nested.iter().map(|n| n.root.clone()).collect();
        let executable = info.get("CFBundleExecutable").and_then(plist::Value::as_string).map(PathBuf::from);
        let mut code = Vec::new();
        for rel in walk_files(path)? {
            let abs = path.join(&rel);
            if nested_roots.iter().any(|n| abs.starts_with(n)) || rel.starts_with("_CodeSignature") {
                continue;
            }
            if abs.is_symlink() || !russet_macho::is_macho_file(&abs).unwrap_or(false) {
                continue;
            }
            let item = if executable.as_deref() == Some(rel.as_path()) {
                if kind == BundleKind::Framework { CodeItem::Framework(rel) } else { CodeItem::MainExecutable(rel) }
            } else if rel.parent() == Some(Path::new("Frameworks"))
                && rel.extension().and_then(|e| e.to_str()) == Some("dylib")
            {
                CodeItem::Dylib(rel)
            } else {
                CodeItem::OtherMachO(rel)
            };
            code.push(item);
        }
        Ok(Self { root: path.to_path_buf(), rel_path, info, kind, nested, code })
    }

    /// `CFBundleIdentifier`.
    pub fn identifier(&self) -> Option<&str> {
        self.info.get("CFBundleIdentifier").and_then(plist::Value::as_string)
    }

    /// `CFBundleExecutable`.
    pub fn executable_name(&self) -> Option<&str> {
        self.info.get("CFBundleExecutable").and_then(plist::Value::as_string)
    }

    /// The main executable's path, when it exists.
    pub fn executable_path(&self) -> Option<PathBuf> {
        let p = self.root.join(self.executable_name()?);
        p.is_file().then_some(p)
    }

    /// A string value of the Info.plist.
    pub fn info_string(&self, key: &str) -> Option<&str> {
        self.info.get(key).and_then(plist::Value::as_string)
    }

    /// This bundle and every nested one, deepest first (the signing order); the top bundle is last.
    pub fn all(&self) -> Vec<&AppBundle> {
        let mut out = Vec::new();
        fn visit<'a>(b: &'a AppBundle, out: &mut Vec<&'a AppBundle>) {
            for n in &b.nested {
                visit(n, out);
            }
            out.push(b);
        }
        visit(self, &mut out);
        // Deeper bundles first; a stable sort keeps the walk order among equal depths.
        out.sort_by_key(|b| std::cmp::Reverse(b.rel_path.matches('/').count() + usize::from(!b.rel_path.is_empty())));
        out
    }

    /// Relative paths of every bundle in signing order (deepest first, `""` = the app, last).
    pub fn signing_order(&self) -> Vec<String> {
        self.all().into_iter().map(|b| b.rel_path.clone()).collect()
    }

    /// `(rel path, CFBundleIdentifier)` of every bundle that has one, in signing order.
    pub fn bundle_ids(&self) -> Vec<(String, String)> {
        self.all().into_iter().filter_map(|b| b.identifier().map(|id| (b.rel_path.clone(), id.to_owned()))).collect()
    }

    /// The bundle at `rel_path` (`""` for this one).
    pub fn find(&self, rel_path: &str) -> Option<&AppBundle> {
        self.all().into_iter().find(|b| b.rel_path == rel_path)
    }

    /// Absolute paths of every Mach-O file in the tree, nested bundles included.
    pub fn macho_files(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> =
            self.all().into_iter().flat_map(|b| b.code.iter().map(|c| b.root.join(c.path()))).collect();
        out.sort();
        out
    }
}
