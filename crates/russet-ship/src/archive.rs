// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `.xcarchive` layout (SIGN §9.1), so a Mac user can open it in Xcode Organizer:
//!
//! ```text
//! <Name>.xcarchive/
//!   Info.plist                 ApplicationProperties{…}, ArchiveVersion=2, CreationDate, Name, SchemeName
//!   Products/Applications/<App>.app
//!   dSYMs/<App>.app.dSYM, …
//! ```
//!
//! The copy preserves symlinks and the executable bit, the `Info.plist` has a fixed `CreationDate`
//! ([`ArchiveRequest::created`]), and [`Archive::digest`] is a SHA-256 over the whole tree in sorted order, so
//! two runs over the same inputs give the same archive (SIGN acceptance: the archive SHA-256 is identical across
//! two runs).

use std::time::SystemTime;

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_sign::AppBundle;
use sha2::{Digest, Sha256};

use crate::{ExportMethod, archive_failed, invalid, utf8};

/// What to archive: a built `.app` and its dSYMs.
#[derive(Debug, Clone)]
pub struct ArchiveRequest {
    /// The built `.app` directory.
    pub app: Utf8PathBuf,
    /// dSYM bundles to copy into `dSYMs/` (from the build, SIGN §9.1).
    pub dsyms: Vec<Utf8PathBuf>,
    /// The archive's name (the scheme or app name); the `.xcarchive` is named `<name>.xcarchive`.
    pub name: String,
    /// The scheme name recorded in `Info.plist` (defaults to `name`).
    pub scheme: Option<String>,
    pub method: ExportMethod,
    /// The signing identity's display name, recorded in `ApplicationProperties`.
    pub signing_identity: Option<String>,
    /// The team ID, recorded in `ApplicationProperties`.
    pub team_id: Option<String>,
    /// The `CreationDate`. Fix it (e.g. to `SOURCE_DATE_EPOCH`) for a byte-reproducible archive.
    pub created: SystemTime,
}

impl ArchiveRequest {
    /// A request for `app` with the given dSYMs, named `name`, created now.
    pub fn new(app: Utf8PathBuf, name: impl Into<String>, method: ExportMethod) -> Self {
        Self {
            app,
            dsyms: Vec::new(),
            name: name.into(),
            scheme: None,
            method,
            signing_identity: None,
            team_id: None,
            created: SystemTime::now(),
        }
    }
}

/// A created `.xcarchive`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    /// The `.xcarchive` directory.
    pub path: Utf8PathBuf,
    /// The archived app, relative to the archive root (`Products/Applications/<App>.app`).
    pub app_rel: String,
    pub bundle_id: String,
    pub short_version: String,
    pub build_version: String,
    /// The dSYM bundles copied into `dSYMs/`.
    pub dsyms: Vec<Utf8PathBuf>,
    /// Lowercase hex SHA-256 over the whole archive tree (sorted), the reproducibility fingerprint.
    pub digest: String,
}

/// Lays out `req` as an `.xcarchive` under `out_dir` (SIGN §9.1) and returns it with its content digest. An
/// existing `<name>.xcarchive` in `out_dir` is replaced.
pub fn create_archive(req: &ArchiveRequest, out_dir: &Utf8Path) -> Result<Archive> {
    if !req.app.is_dir() {
        return Err(invalid(format!("the build product {} is not a directory", req.app)));
    }
    let app_name = req
        .app
        .file_name()
        .filter(|n| n.ends_with(".app"))
        .ok_or_else(|| invalid(format!("{} is not a .app bundle", req.app)))?
        .to_owned();

    let bundle = AppBundle::open(req.app.as_std_path())?;
    let bundle_id =
        bundle.identifier().ok_or_else(|| invalid(format!("{} has no CFBundleIdentifier", req.app)))?.to_owned();
    let short_version = bundle.info_string("CFBundleShortVersionString").unwrap_or("1.0").to_owned();
    let build_version = bundle.info_string("CFBundleVersion").unwrap_or("1").to_owned();

    let archive = out_dir.join(format!("{}.xcarchive", req.name));
    if archive.exists() {
        std::fs::remove_dir_all(&archive).map_err(|e| archive_failed(format!("cannot replace {archive}: {e}")))?;
    }
    let products = archive.join("Products").join("Applications");
    std::fs::create_dir_all(&products).map_err(|e| archive_failed(format!("cannot create {products}: {e}")))?;
    let app_dest = products.join(&app_name);
    copy_tree(&req.app, &app_dest)?;

    let mut dsyms = Vec::new();
    if !req.dsyms.is_empty() {
        let dsym_dir = archive.join("dSYMs");
        std::fs::create_dir_all(&dsym_dir).map_err(|e| archive_failed(format!("cannot create {dsym_dir}: {e}")))?;
        for src in &req.dsyms {
            if !src.exists() {
                return Err(archive_failed(format!("dSYM {src} does not exist")));
            }
            let name = src.file_name().ok_or_else(|| archive_failed(format!("{src} has no file name")))?;
            let dest = dsym_dir.join(name);
            copy_tree(src, &dest)?;
            dsyms.push(Utf8PathBuf::from(format!("dSYMs/{name}")));
        }
    }

    let app_rel = format!("Products/Applications/{app_name}");
    write_info_plist(req, &archive, &app_rel, &bundle_id, &short_version, &build_version)?;

    let digest = tree_digest(&archive)?;
    Ok(Archive { path: archive, app_rel, bundle_id, short_version, build_version, dsyms, digest })
}

/// Writes the archive `Info.plist` (SIGN §9.1). The `CreationDate` is [`ArchiveRequest::created`], so the file is
/// identical across runs when the caller fixes it.
fn write_info_plist(
    req: &ArchiveRequest,
    archive: &Utf8Path,
    app_rel: &str,
    bundle_id: &str,
    short_version: &str,
    build_version: &str,
) -> Result<()> {
    let mut app_props = plist::Dictionary::new();
    app_props.insert("ApplicationPath".into(), plist::Value::String(app_rel.to_owned()));
    app_props.insert("CFBundleIdentifier".into(), plist::Value::String(bundle_id.to_owned()));
    app_props.insert("CFBundleShortVersionString".into(), plist::Value::String(short_version.to_owned()));
    app_props.insert("CFBundleVersion".into(), plist::Value::String(build_version.to_owned()));
    if let Some(id) = &req.signing_identity {
        app_props.insert("SigningIdentity".into(), plist::Value::String(id.clone()));
    }
    if let Some(team) = &req.team_id {
        app_props.insert("Team".into(), plist::Value::String(team.clone()));
    }

    let mut root = plist::Dictionary::new();
    root.insert("ApplicationProperties".into(), plist::Value::Dictionary(app_props));
    root.insert("ArchiveVersion".into(), plist::Value::Integer(2.into()));
    root.insert("CreationDate".into(), plist::Value::Date(req.created.into()));
    root.insert("Name".into(), plist::Value::String(req.name.clone()));
    root.insert("SchemeName".into(), plist::Value::String(req.scheme.clone().unwrap_or_else(|| req.name.clone())));

    let path = archive.join("Info.plist");
    plist::Value::Dictionary(root)
        .to_file_xml(&path)
        .map_err(|e| archive_failed(format!("cannot write {path}: {e}")))?;
    Ok(())
}

/// Copies a file, directory tree or symlink from `src` to `dest`, preserving symlinks and the executable bit.
fn copy_tree(src: &Utf8Path, dest: &Utf8Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(src).map_err(|e| archive_failed(format!("cannot read {src}: {e}")))?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(src).map_err(|e| archive_failed(format!("cannot read link {src}: {e}")))?;
        symlink(&target, dest.as_std_path())
            .map_err(|e| archive_failed(format!("cannot create symlink {dest}: {e}")))?;
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::create_dir_all(dest).map_err(|e| archive_failed(format!("cannot create {dest}: {e}")))?;
        let mut entries: Vec<_> = std::fs::read_dir(src)
            .map_err(|e| archive_failed(format!("cannot read {src}: {e}")))?
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| archive_failed(format!("cannot read {src}: {e}")))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(|| invalid(format!("non-UTF-8 name under {src}")))?;
            copy_tree(&src.join(name), &dest.join(name))?;
        }
        return Ok(());
    }
    std::fs::copy(src, dest).map_err(|e| archive_failed(format!("cannot copy {src} to {dest}: {e}")))?;
    preserve_exec(&meta, dest)?;
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, dest)
}

#[cfg(windows)]
fn symlink(target: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    // On Windows the archive keeps the link's text as a plain file (symlink creation needs privilege); the IPA
    // writer re-materializes symlinks from their stored data.
    std::fs::write(dest, target.to_string_lossy().as_bytes())
}

#[cfg(unix)]
fn preserve_exec(meta: &std::fs::Metadata, dest: &Utf8Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode();
    let want = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
    std::fs::set_permissions(dest, std::fs::Permissions::from_mode(want))
        .map_err(|e| archive_failed(format!("cannot set the mode of {dest}: {e}")))
}

#[cfg(not(unix))]
fn preserve_exec(_meta: &std::fs::Metadata, _dest: &Utf8Path) -> Result<()> {
    Ok(())
}

/// A SHA-256 over the whole tree under `root`, visiting entries in sorted order: each entry contributes its
/// relative path, a type tag and its bytes (file contents, or a symlink's target). Independent of mtimes and the
/// filesystem's iteration order, so it is stable across runs and hosts.
fn tree_digest(root: &Utf8Path) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut rel = String::new();
    digest_into(root, &mut rel, &mut hasher)?;
    Ok(russet_sign::hex(&hasher.finalize()))
}

fn digest_into(path: &Utf8Path, rel: &mut String, hasher: &mut Sha256) -> Result<()> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| archive_failed(format!("cannot read {path}: {e}")))?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path).map_err(|e| archive_failed(format!("cannot read link {path}: {e}")))?;
        hasher.update(b"L\0");
        hasher.update(rel.as_bytes());
        hasher.update([0]);
        hasher.update(target.to_string_lossy().as_bytes());
        hasher.update([0]);
        return Ok(());
    }
    if meta.is_dir() {
        hasher.update(b"D\0");
        hasher.update(rel.as_bytes());
        hasher.update([0]);
        let mut names: Vec<String> = std::fs::read_dir(path)
            .map_err(|e| archive_failed(format!("cannot read {path}: {e}")))?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| archive_failed(format!("cannot read {path}: {e}")))?;
        names.sort();
        for name in names {
            let base = rel.len();
            if !rel.is_empty() {
                rel.push('/');
            }
            rel.push_str(&name);
            digest_into(&path.join(&name), rel, hasher)?;
            rel.truncate(base);
        }
        return Ok(());
    }
    let bytes = std::fs::read(path).map_err(|e| archive_failed(format!("cannot read {path}: {e}")))?;
    hasher.update(b"F\0");
    hasher.update(rel.as_bytes());
    hasher.update([0]);
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
    Ok(())
}

/// The `.app` under an archive's `Products/Applications/`, if there is exactly one (used by `ship.ipa`).
pub fn archive_app(archive: &Utf8Path) -> Result<Utf8PathBuf> {
    let dir = archive.join("Products").join("Applications");
    let apps: Vec<Utf8PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| invalid(format!("{archive} has no Products/Applications ({e})")))?
        .filter_map(|e| e.ok())
        .filter_map(|e| utf8(&e.path()).ok().map(Utf8Path::to_owned))
        .filter(|p| p.extension() == Some("app"))
        .collect();
    let n = apps.len();
    let mut it = apps.into_iter();
    match (it.next(), it.next()) {
        (Some(app), None) => Ok(app),
        (None, _) => Err(invalid(format!("{archive} contains no .app under Products/Applications"))),
        _ => Err(invalid(format!("{archive} contains {n} apps under Products/Applications; expected one"))),
    }
}
