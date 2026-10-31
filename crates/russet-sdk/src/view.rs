// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The platform view (ARCHITECTURE §16.3; build §3.4): a Russet-owned directory that mirrors the SDK's
//! `Developer/` layout and adds Russet's tools, passed to Swift Build as `XCODE_EXTRA_PLATFORM_FOLDERS`. Swift Build
//! resolves `actool` and friends only through `<platform>/usr/local/bin` and its siblings, never `PATH` [V build
//! V7–V8].
//!
//! ```text
//! data/sdk/<id>/view/Developer/
//!   Toolchains -> <bundle>/Developer/Toolchains
//!   Platforms/iPhoneOS.platform/
//!     Info.plist -> <bundle>/Developer/Platforms/iPhoneOS.platform/Info.plist
//!     version.plist -> <the platform version.plist, wherever the bundle keeps it>
//!     Developer -> <bundle>/Developer/Platforms/iPhoneOS.platform/Developer
//!     usr/local/bin/<tool> -> russet-tool   (W16 ships russet-tool; the directory exists from Wave 1)
//! ```
//!
//! The view is additive: Apple files and the bundle are never modified. It is rebuilt atomically when its
//! recorded links (`data/sdk/<id>/view.json`) differ. Linux and macOS use symlinks; Windows junctions and `.exe`
//! copies are W27b's.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
#[cfg(not(unix))]
use russet_core::error::{Error, codes};

use crate::facts::BundleFacts;
use crate::toolset::io_error;

/// Tool names Swift Build looks up in the platform's `usr/local/bin` (build §3.4).
pub const PLATFORM_TOOLS: &[&str] =
    &["actool", "ibtool", "xcstringstool", "momc", "mapc", "metal", "metallib", "appintentsmetadataprocessor"];

static NONCE: AtomicU64 = AtomicU64::new(0);

/// The links of a view: path relative to the view root → absolute target.
pub fn plan(facts: &BundleFacts, tool: Option<&Utf8Path>) -> BTreeMap<String, Utf8PathBuf> {
    let mut links = BTreeMap::new();
    let p = "Developer/Platforms/iPhoneOS.platform";
    links.insert("Developer/Toolchains".to_owned(), facts.toolchains_dir.clone());
    links.insert(format!("{p}/Info.plist"), facts.platform_dir.join("Info.plist"));
    links.insert(format!("{p}/Developer"), facts.platform_dir.join("Developer"));
    if let Some(v) = &facts.platform_version_plist {
        links.insert(format!("{p}/version.plist"), v.clone());
    }
    if let Some(tool) = tool {
        for name in PLATFORM_TOOLS {
            links.insert(format!("{p}/usr/local/bin/{name}"), tool.to_owned());
        }
    }
    links
}

/// Makes `sdk_dir/view` match `links`, rebuilding it atomically when needed; returns
/// `sdk_dir/view/Developer/Platforms`.
pub fn materialize(sdk_dir: &Utf8Path, links: &BTreeMap<String, Utf8PathBuf>) -> Result<Utf8PathBuf> {
    let view = sdk_dir.join("view");
    let marker = sdk_dir.join("view.json");
    let platforms = view.join("Developer/Platforms");
    let wanted = serde_json::to_string_pretty(&serde_json::json!({"schema": 1, "links": links})).unwrap_or_default();
    if view.is_dir() && std::fs::read_to_string(&marker).is_ok_and(|m| m == wanted) {
        return Ok(platforms);
    }
    std::fs::create_dir_all(sdk_dir).map_err(|e| io_error(sdk_dir, e))?;
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = sdk_dir.join(format!("view.tmp-{}-{nonce}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let built = build(&tmp, links);
    if let Err(e) = built {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    // Replace the old view. A concurrent resolver may have won the race; then keep its (identical) view.
    let old = sdk_dir.join(format!("view.old-{}-{nonce}", std::process::id()));
    if view.exists() {
        let _ = std::fs::rename(&view, &old);
    }
    match std::fs::rename(&tmp, &view) {
        Ok(()) => {}
        Err(_) if view.is_dir() => {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(io_error(&view, e));
        }
    }
    let _ = std::fs::remove_dir_all(&old);
    let marker_tmp = sdk_dir.join(format!("view.json.tmp-{}-{nonce}", std::process::id()));
    std::fs::write(&marker_tmp, wanted).map_err(|e| io_error(&marker_tmp, e))?;
    std::fs::rename(&marker_tmp, &marker).map_err(|e| io_error(&marker, e))?;
    Ok(platforms)
}

fn build(root: &Utf8Path, links: &BTreeMap<String, Utf8PathBuf>) -> Result<()> {
    std::fs::create_dir_all(root.join("Developer/Platforms/iPhoneOS.platform/usr/local/bin"))
        .map_err(|e| io_error(root, e))?;
    for (rel, target) in links {
        let link = root.join(rel);
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        symlink(target, &link)?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Utf8Path, link: &Utf8Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|e| io_error(link, e))
}

#[cfg(not(unix))]
fn symlink(_target: &Utf8Path, link: &Utf8Path) -> Result<()> {
    Err(Error::new(
        codes::core::UNSUPPORTED_PLATFORM,
        format!("cannot create {link}: the Windows platform view (junctions) lands with W27b"),
    )
    .with_hint("Build on Linux for now (PLAN §8.2)."))
}
