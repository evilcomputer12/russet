// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Bundle assembly (build §4.6, B06 subset): the `.app` layout, nested-bundle DT normalization, Swift
//! back-deployment, rpath hygiene through `russet-macho`, fixed timestamps, and the [`SigningPlan`].
//!
//! ```text
//! <App>.app/
//!   <App>                      executable (the builder's <App>-App product)
//!   Info.plist                 binary
//!   PkgInfo                    "APPL????"
//!   <Pkg>_<Target>.bundle/     SwiftPM resource bundles (DT-normalized)
//!   PrivacyInfo.xcprivacy      if configured
//!   <icon>.png                 a loose development icon, if configured
//!   Frameworks/<X>.framework/  package frameworks and binary-target frameworks
//!   Frameworks/libswift*.dylib back-deployment Swift libraries (only when referenced and needed)
//!   PlugIns/<Ext>.appex/       extensions (executable, Info.plist, resource bundles)
//! ```
//!
//! The bundle is assembled in a staging directory and swapped in at the end, so a failed build leaves the previous
//! bundle in place. Every file and directory gets the same modification time (`SOURCE_DATE_EPOCH`, else
//! 2001-01-01), so two builds of the same sources give byte-identical bundles before signing (T-BUNDLE-03).

use std::collections::BTreeMap;
use std::time::SystemTime;

use camino::{Utf8Path, Utf8PathBuf};
use plist::{Dictionary, Value};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_sdk::Version;
use uuid::Uuid;

use crate::events::{BuildEvent, Emitter, Severity};
use crate::macho::{MachOSummary, MachOTool, is_host_rpath};
use crate::stack::swiftpm::{ProductPlan, io};
use crate::{BundleInfo, BundleKind, LintFinding, LintSeverity, SignItem, SignKind, SigningPlan, metadata};

/// One product to put in the bundle.
#[derive(Debug, Clone)]
pub struct ProductBundle {
    pub plan: ProductPlan,
    pub info: Dictionary,
    /// The entitlements file to sign with (written under `.russet/work/entitlements/`).
    pub entitlements: Option<Utf8PathBuf>,
    pub bundle_id: String,
    pub original_bundle_id: String,
    pub version: String,
    pub build_number: String,
}

/// Everything assembly needs.
pub struct AssemblyInput<'a> {
    pub products_dir: &'a Utf8Path,
    /// `.russet/build/<profile>-iphoneos`.
    pub out_dir: &'a Utf8Path,
    pub app: &'a ProductBundle,
    pub extensions: &'a [ProductBundle],
    pub privacy_manifest: Option<&'a Utf8Path>,
    pub icon_png: Option<&'a Utf8Path>,
    /// The app's DT* keys, applied to every nested bundle Russet built.
    pub dt: &'a Dictionary,
    pub deployment_target: Version,
    pub back_deploy_dirs: &'a [Utf8PathBuf],
    pub back_deploy_not_needed_from: &'a BTreeMap<String, Version>,
    /// Debug builds sign with `get-task-allow`.
    pub debug: bool,
    pub epoch: SystemTime,
    pub macho: &'a dyn MachOTool,
}

/// One Mach-O file of the bundle, for the build report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachOEntry {
    /// Relative to the bundle's parent directory (`RussetProbe.app/RussetProbe`).
    pub path: String,
    pub summary: MachOSummary,
    pub built_by_russet: bool,
}

/// What assembly produced.
#[derive(Debug, Clone)]
pub struct Assembled {
    pub app: BundleInfo,
    pub extensions: Vec<BundleInfo>,
    pub signing_plan: SigningPlan,
    pub lints: Vec<LintFinding>,
    pub machos: Vec<MachOEntry>,
    pub uuids: Vec<Uuid>,
}

const PKG_INFO: &[u8] = b"APPL????";

/// Copies a file, keeping its permissions.
fn copy_file(from: &Utf8Path, to: &Utf8Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    std::fs::copy(from, to).map_err(|e| io(from, e))?;
    Ok(())
}

/// Copies a directory tree (files, directories and symlinks as links).
pub fn copy_tree(from: &Utf8Path, to: &Utf8Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|e| io(to, e))?;
    let mut entries: Vec<_> = from.read_dir_utf8().map_err(|e| io(from, e))?.filter_map(|e| e.ok()).collect();
    entries.sort_by(|a, b| a.file_name().cmp(b.file_name()));
    for entry in entries {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| io(src, e))?;
        if kind.is_symlink() {
            let target = std::fs::read_link(src).map_err(|e| io(src, e))?;
            symlink(&target, &dst)?;
        } else if kind.is_dir() {
            copy_tree(src, &dst)?;
        } else {
            copy_file(src, &dst)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &std::path::Path, link: &Utf8Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|e| io(link, e))
}

#[cfg(not(unix))]
fn symlink(target: &std::path::Path, link: &Utf8Path) -> Result<()> {
    // Windows bundles never contain links (W27b); copy the target instead.
    std::fs::copy(target, link).map(|_| ()).map_err(|e| io(link, e))
}

#[cfg(unix)]
fn make_executable(path: &Utf8Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|e| io(path, e))
}

#[cfg(not(unix))]
fn make_executable(_path: &Utf8Path) -> Result<()> {
    Ok(())
}

fn write_binary_plist(path: &Utf8Path, d: &Dictionary) -> Result<()> {
    let bytes = russet_plist::to_binary(&Value::Dictionary(d.clone()))
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot write {path}: {e}")))?;
    std::fs::write(path, bytes).map_err(|e| io(path, e))
}

/// Rewrites the DT* keys of a nested bundle's Info.plist in place, keeping its encoding.
fn normalize_nested(info_path: &Utf8Path, dt: &Dictionary) -> Result<bool> {
    if !info_path.is_file() {
        return Ok(false);
    }
    let bytes = std::fs::read(info_path).map_err(|e| io(info_path, e))?;
    let (value, format) = russet_plist::from_bytes(&bytes)
        .map_err(|e| Error::new(codes::build::COMPILE_FAILED, format!("{info_path}: {e}")))?;
    let Value::Dictionary(mut d) = value else { return Ok(false) };
    if !metadata::normalize(&mut d, dt) {
        return Ok(false);
    }
    let format = if format == russet_plist::Format::OpenStep { russet_plist::Format::Binary } else { format };
    let out = russet_plist::to_bytes(&Value::Dictionary(d), format)
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot write {info_path}: {e}")))?;
    std::fs::write(info_path, out).map_err(|e| io(info_path, e))?;
    Ok(true)
}

/// Whether `path` starts with the static-archive magic (`!<arch>` or `!<thin>`): such binary targets link
/// statically and are not embedded (xtool's rule).
fn is_static_archive(path: &Utf8Path) -> bool {
    std::fs::read(path).is_ok_and(|b| b.starts_with(b"!<arch>\n") || b.starts_with(b"!<thin>\n"))
}

fn is_macho_file(path: &Utf8Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 8];
    std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).is_ok_and(|n| russet_ipa::is_macho(&head[..n]))
}

/// Adds one product (executable, Info.plist, resources, frameworks) at `dir`.
fn place_product(
    input: &AssemblyInput<'_>,
    p: &ProductBundle,
    dir: &Utf8Path,
    emitter: &Emitter<'_>,
) -> Result<Vec<Utf8PathBuf>> {
    let mut built = Vec::new();
    let products = input.products_dir;
    let exe_src = products.join(&p.plan.target_name);
    if !exe_src.is_file() {
        return Err(Error::new(codes::build::LINK_FAILED, format!("the build produced no {exe_src}"))
            .with_hint("The builder package's executable target did not link; see the build log."));
    }
    let exe = dir.join(&p.plan.product);
    copy_file(&exe_src, &exe)?;
    make_executable(&exe)?;
    built.push(exe);
    write_binary_plist(&dir.join("Info.plist"), &p.info)?;
    for bundle in &p.plan.resource_bundles {
        let src = products.join(bundle);
        if !src.is_dir() {
            emitter.warn("build.warn.resources", format!("the resource bundle {bundle} was not built"), None);
            continue;
        }
        let dst = dir.join(bundle);
        copy_tree(&src, &dst)?;
        normalize_nested(&dst.join("Info.plist"), input.dt)?;
    }
    for name in &p.plan.binary_frameworks {
        let src = products.join(format!("{name}.framework"));
        if !src.is_dir() || is_static_archive(&src.join(name)) {
            continue;
        }
        copy_tree(&src, &dir.join("Frameworks").join(format!("{name}.framework")))?;
    }
    for lib in &p.plan.dynamic_libraries {
        let src = products.join(format!("lib{lib}.dylib"));
        if src.is_file() {
            let dst = dir.join("Frameworks").join(format!("lib{lib}.dylib"));
            copy_file(&src, &dst)?;
            built.push(dst);
            emitter.warn(
                "build.warn.lint",
                format!("lib{lib}.dylib is embedded as a loose dylib; wrapping it in a framework lands with B04 (W16)"),
                Some("The App Store rejects loose non-Swift dylibs (lint L008); development installs work."),
            );
        }
    }
    Ok(built)
}

/// Assembles the app; returns where everything went.
pub fn assemble(input: &AssemblyInput<'_>, emitter: &Emitter<'_>) -> Result<Assembled> {
    let app_name = format!("{}.app", input.app.plan.product);
    let final_app = input.out_dir.join(&app_name);
    let staging = input.out_dir.join(format!(".{app_name}.staging"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| io(&staging, e))?;
    }
    std::fs::create_dir_all(&staging).map_err(|e| io(&staging, e))?;
    let result = assemble_into(input, &staging, &final_app, emitter);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn assemble_into(
    input: &AssemblyInput<'_>,
    staging: &Utf8Path,
    final_app: &Utf8Path,
    emitter: &Emitter<'_>,
) -> Result<Assembled> {
    let mut built = place_product(input, input.app, staging, emitter)?;
    std::fs::write(staging.join("PkgInfo"), PKG_INFO).map_err(|e| io(staging, e))?;
    let package_frameworks = input.products_dir.join("PackageFrameworks");
    if let Ok(entries) = package_frameworks.read_dir_utf8() {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_owned())
            .filter(|n| n.ends_with(".framework"))
            .collect();
        names.sort();
        for name in names {
            let dst = staging.join("Frameworks").join(&name);
            copy_tree(&package_frameworks.join(&name), &dst)?;
            normalize_nested(&dst.join("Info.plist"), input.dt)?;
            let binary = dst.join(name.trim_end_matches(".framework"));
            if binary.is_file() {
                built.push(binary);
            }
        }
    }
    if let Some(icon) = input.icon_png {
        let name = icon.file_name().unwrap_or("Icon.png");
        copy_file(icon, &staging.join(name))?;
    }
    if let Some(privacy) = input.privacy_manifest {
        copy_file(privacy, &staging.join("PrivacyInfo.xcprivacy"))?;
    }
    for ext in input.extensions {
        let dir = staging.join("PlugIns").join(format!("{}.appex", ext.plan.product));
        std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
        built.extend(place_product(input, ext, &dir, emitter)?);
        normalize_nested(&dir.join("Info.plist"), input.dt)?;
    }

    let lints = rpath_hygiene(input, &built, staging)?;
    back_deploy(input, &built, staging, emitter)?;

    // Report every Mach-O file of the bundle.
    let mut machos = Vec::new();
    let mut uuids = Vec::new();
    let parent_len = staging.as_str().len() + 1;
    for file in walk_files(staging)? {
        if !is_macho_file(&file) {
            continue;
        }
        let bytes = std::fs::read(&file).map_err(|e| io(&file, e))?;
        let summary = input.macho.summary(&bytes)?;
        if let Some(u) = summary.uuid {
            uuids.push(u);
        }
        let rel = format!(
            "{}/{}",
            final_app.file_name().unwrap_or_default(),
            &file.as_str()[parent_len.min(file.as_str().len())..]
        );
        machos.push(MachOEntry { path: rel, built_by_russet: built.contains(&file), summary });
    }

    set_times(staging, input.epoch)?;

    // Swap the staging bundle in.
    if final_app.exists() {
        let old = final_app.with_file_name(format!(".{}.old", final_app.file_name().unwrap_or_default()));
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(final_app, &old).map_err(|e| io(final_app, e))?;
        std::fs::rename(staging, final_app).map_err(|e| io(final_app, e))?;
        let _ = std::fs::remove_dir_all(&old);
    } else {
        std::fs::rename(staging, final_app).map_err(|e| io(final_app, e))?;
    }

    let info = |p: &ProductBundle, path: Utf8PathBuf, kind: BundleKind| BundleInfo {
        executable: path.join(&p.plan.product),
        path,
        bundle_id: p.bundle_id.clone(),
        original_bundle_id: p.original_bundle_id.clone(),
        kind,
        product: p.plan.product.clone(),
        version: p.version.clone(),
        build_number: p.build_number.clone(),
    };
    let app = info(input.app, final_app.to_owned(), BundleKind::App);
    let extensions: Vec<BundleInfo> = input
        .extensions
        .iter()
        .map(|e| info(e, final_app.join("PlugIns").join(format!("{}.appex", e.plan.product)), BundleKind::AppExtension))
        .collect();
    let signing_plan = signing_plan(final_app, input, &extensions)?;
    for e in &lints {
        emitter.emit(BuildEvent::Diagnostic {
            diagnostic: crate::events::Diagnostic {
                severity: Severity::Note,
                message: e.message.clone(),
                file: e.path.as_ref().map(|p| p.to_string()),
                line: None,
                column: None,
                tool: Some("russet".into()),
                code: Some(e.code.clone()),
            },
        });
    }
    Ok(Assembled { app, extensions, signing_plan, lints, machos, uuids })
}

/// Deletes host `LC_RPATH`s from every Mach-O Russet built (lint L006, auto-fixed).
fn rpath_hygiene(input: &AssemblyInput<'_>, built: &[Utf8PathBuf], staging: &Utf8Path) -> Result<Vec<LintFinding>> {
    let mut lints = Vec::new();
    for file in built {
        let bytes = std::fs::read(file).map_err(|e| io(file, e))?;
        let summary = input.macho.summary(&bytes)?;
        for rpath in summary.rpaths.iter().filter(|r| is_host_rpath(r)) {
            input.macho.delete_rpath(file, rpath)?;
            lints.push(LintFinding {
                code: "L006".into(),
                severity: LintSeverity::Warning,
                path: file.strip_prefix(staging).ok().map(Utf8Path::to_owned),
                message: format!("removed the host rpath {rpath}"),
                fixed: true,
            });
        }
    }
    Ok(lints)
}

/// Copies the back-deployment Swift libraries the binaries load through `@rpath` and the deployment target
/// still needs (`SwiftCompatibilityLibraryNotNeededStartingIn`).
fn back_deploy(
    input: &AssemblyInput<'_>,
    built: &[Utf8PathBuf],
    staging: &Utf8Path,
    emitter: &Emitter<'_>,
) -> Result<()> {
    let mut needed = std::collections::BTreeSet::new();
    for file in built {
        let bytes = std::fs::read(file).map_err(|e| io(file, e))?;
        for dylib in input.macho.summary(&bytes)?.dylibs {
            if let Some(name) =
                dylib.strip_prefix("@rpath/").filter(|n| n.starts_with("libswift") && n.ends_with(".dylib"))
            {
                needed.insert(name.to_owned());
            }
        }
    }
    for name in needed {
        if input.back_deploy_not_needed_from.get(&name).is_some_and(|from| input.deployment_target >= *from) {
            continue;
        }
        let Some(src) = input.back_deploy_dirs.iter().map(|d| d.join(&name)).find(|p| p.is_file()) else {
            return Err(Error::new(codes::sdk::INCOMPLETE, format!("the app loads @rpath/{name}, which the SDK bundle does not have"))
                .with_hint("Use a complete SDK (its swift-*/iphoneos back-deployment directories), or raise the deployment target."));
        };
        copy_file(&src, &staging.join("Frameworks").join(&name))?;
        emitter.emit(BuildEvent::Log {
            stream: russet_core::progress::LogStream::Info,
            line: format!("embedded the back-deployment library {name}"),
        });
    }
    Ok(())
}

fn walk_files(root: &Utf8Path) -> Result<Vec<Utf8PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in dir.read_dir_utf8().map_err(|e| io(&dir, e))? {
            let entry = entry.map_err(|e| io(&dir, e))?;
            let kind = entry.file_type().map_err(|e| io(entry.path(), e))?;
            if kind.is_dir() {
                stack.push(entry.path().to_owned());
            } else if kind.is_file() {
                out.push(entry.path().to_owned());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Sets every file and directory under `root` (and `root`) to `epoch`.
pub fn set_times(root: &Utf8Path, epoch: SystemTime) -> Result<()> {
    let t = filetime::FileTime::from_system_time(epoch);
    let mut dirs = vec![root.to_owned()];
    let mut all_dirs = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in dir.read_dir_utf8().map_err(|e| io(&dir, e))? {
            let entry = entry.map_err(|e| io(&dir, e))?;
            let kind = entry.file_type().map_err(|e| io(entry.path(), e))?;
            if kind.is_symlink() {
                filetime::set_symlink_file_times(entry.path(), t, t).map_err(|e| io(entry.path(), e))?;
            } else if kind.is_dir() {
                dirs.push(entry.path().to_owned());
            } else {
                filetime::set_file_times(entry.path(), t, t).map_err(|e| io(entry.path(), e))?;
            }
        }
        all_dirs.push(dir);
    }
    // Directories last, deepest first: setting a child's time does not touch the parent, but creating did.
    for dir in all_dirs.iter().rev() {
        filetime::set_file_times(dir, t, t).map_err(|e| io(dir, e))?;
    }
    Ok(())
}

/// Inside-out signing order: nested frameworks and dylibs, then each extension (its own nested code first), then
/// the app.
fn signing_plan(app: &Utf8Path, input: &AssemblyInput<'_>, extensions: &[BundleInfo]) -> Result<SigningPlan> {
    let mut items = Vec::new();
    let nested = |bundle: &Utf8Path, items: &mut Vec<SignItem>| -> Result<()> {
        let frameworks = bundle.join("Frameworks");
        let Ok(entries) = frameworks.read_dir_utf8() else { return Ok(()) };
        let mut paths: Vec<Utf8PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path().to_owned()).collect();
        paths.sort();
        for p in paths {
            let kind = if p.extension() == Some("framework") {
                SignKind::Bundle
            } else if p.extension() == Some("dylib") {
                SignKind::MachO
            } else {
                continue;
            };
            items.push(SignItem { path: p, kind, entitlements: None, get_task_allow: false });
        }
        Ok(())
    };
    for (ext, bundle) in input.extensions.iter().zip(extensions) {
        nested(&bundle.path, &mut items)?;
        items.push(SignItem {
            path: bundle.path.clone(),
            kind: SignKind::Bundle,
            entitlements: ext.entitlements.clone(),
            get_task_allow: input.debug,
        });
    }
    nested(app, &mut items)?;
    items.push(SignItem {
        path: app.to_owned(),
        kind: SignKind::Bundle,
        entitlements: input.app.entitlements.clone(),
        get_task_allow: input.debug,
    });
    Ok(SigningPlan { items })
}
