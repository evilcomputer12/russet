// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Apps: install, uninstall, list (DEV-06; device.md §6.4).
//!
//! Install is AFC upload to `PublicStaging` plus `installation_proxy Install`, both over lockdown (usbmux), which is
//! faster than the tunnel and survives tunnel resets. A `.app` directory is mirrored file by file (symlinks become
//! AFC symlinks, unlike idevice's helper, which skips them: device.md F12) until W5's `russet-ipa` writer is used
//! for every install; an `.ipa` is uploaded as one file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use idevice::Idevice;
use idevice::afc::AfcClient;
use idevice::afc::opcode::{AfcFopenMode, LinkType};
use idevice::installation_proxy::InstallationProxyClient;
use russet_core::Result;
use russet_core::error::codes;
use russet_core::progress::ProgressSink;

use crate::coredevice::{CoreDeviceClient, feature, service, uninstall_input};
use crate::error::{device_error, from_idevice, invalid_argument};
use crate::manager::DeviceHandle;
use crate::pairing::LABEL;

/// The AFC staging directory installd reads from.
pub const PUBLIC_STAGING: &str = "PublicStaging";

/// Install options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallOptions {
    /// Extra `installation_proxy` client options (rarely needed).
    pub client_options: BTreeMap<String, String>,
}

/// What an install produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    pub bundle_id: String,
    /// The app's path on the device, when installd reports it.
    pub installed_path: Option<String>,
    pub duration: Duration,
    /// Bytes uploaded.
    pub bytes: u64,
}

/// Which apps to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppFilter {
    /// `user` (default) or every app.
    pub all: bool,
    pub bundle_id_prefix: Option<String>,
}

/// One installed app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub bundle_id: String,
    pub name: Option<String>,
    /// `CFBundleShortVersionString`.
    pub version: Option<String>,
    /// `CFBundleVersion`.
    pub build: Option<String>,
    /// The `.app` path on the device.
    pub path: Option<String>,
    /// `CFBundleExecutable`.
    pub executable: Option<String>,
    /// `User` apps signed with a development profile.
    pub developer_app: bool,
    pub removable: bool,
    pub hidden: bool,
    /// `User` or `System`.
    pub application_type: Option<String>,
}

/// `Info.plist` values Russet reads from a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleMeta {
    pub bundle_id: String,
    /// `Payload/<Name>.app` or the directory name.
    pub app_name: String,
}

/// Reads `CFBundleIdentifier` from a `.app` directory.
pub fn read_app_dir(app: &Path) -> Result<BundleMeta> {
    let info = app.join("Info.plist");
    let v: plist::Value = plist::from_file(&info).map_err(|e| {
        invalid_argument(format!("{}: cannot read Info.plist: {e}", app.display()))
            .with_hint("Pass a built .app directory or an .ipa.")
    })?;
    let bundle_id = v
        .as_dictionary()
        .and_then(|d| d.get("CFBundleIdentifier"))
        .and_then(|b| b.as_string())
        .ok_or_else(|| invalid_argument(format!("{}: Info.plist has no CFBundleIdentifier", app.display())))?;
    let app_name = app.file_name().and_then(|n| n.to_str()).unwrap_or("App.app").to_owned();
    Ok(BundleMeta { bundle_id: bundle_id.to_owned(), app_name })
}

/// Reads `CFBundleIdentifier` from `Payload/<Name>.app/Info.plist` inside an `.ipa`.
pub fn read_ipa(ipa: &Path) -> Result<BundleMeta> {
    let bad = |m: String| invalid_argument(m).with_hint("Pass an .ipa with Payload/<Name>.app/Info.plist.");
    let f = std::fs::File::open(ipa).map_err(|e| bad(format!("{}: {e}", ipa.display())))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| bad(format!("{}: not a zip archive: {e}", ipa.display())))?;
    let name = zip
        .file_names()
        .filter_map(|n| n.ok().map(std::borrow::Cow::into_owned))
        .find(|n| {
            let parts: Vec<&str> = n.split('/').collect();
            parts.len() == 3 && parts[0] == "Payload" && parts[1].ends_with(".app") && parts[2] == "Info.plist"
        })
        .ok_or_else(|| bad(format!("{}: no Payload/<Name>.app/Info.plist", ipa.display())))?;
    let app_name = name.split('/').nth(1).unwrap_or("App.app").to_owned();
    let mut entry = zip.by_name(&name).map_err(|e| bad(format!("{}: {e}", ipa.display())))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|e| bad(format!("{}: {e}", ipa.display())))?;
    let v: plist::Value = plist::from_bytes(&bytes).map_err(|e| bad(format!("{name}: {e}")))?;
    let bundle_id = v
        .as_dictionary()
        .and_then(|d| d.get("CFBundleIdentifier"))
        .and_then(|b| b.as_string())
        .ok_or_else(|| bad(format!("{name}: no CFBundleIdentifier")))?;
    Ok(BundleMeta { bundle_id: bundle_id.to_owned(), app_name })
}

/// One upload step of a `.app` directory, in walk order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadItem {
    Dir(String),
    File { local: PathBuf, remote: String, size: u64 },
    Symlink { target: String, remote: String },
}

/// Plans the AFC upload of a `.app` directory below `remote_root` (directories before their contents; symlinks
/// kept as symlinks).
pub fn plan_dir_upload(local: &Path, remote_root: &str) -> Result<Vec<UploadItem>> {
    let mut out = vec![UploadItem::Dir(remote_root.to_owned())];
    let mut queue = std::collections::VecDeque::from([(local.to_path_buf(), remote_root.to_owned())]);
    while let Some((dir, remote)) = queue.pop_front() {
        let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&dir)
            .map_err(|e| invalid_argument(format!("{}: {e}", dir.display())))?
            .filter_map(std::result::Result::ok)
            .collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let child = format!("{remote}/{name}");
            let meta =
                std::fs::symlink_metadata(&path).map_err(|e| invalid_argument(format!("{}: {e}", path.display())))?;
            if meta.file_type().is_symlink() {
                let target =
                    std::fs::read_link(&path).map_err(|e| invalid_argument(format!("{}: {e}", path.display())))?;
                out.push(UploadItem::Symlink { target: target.to_string_lossy().into_owned(), remote: child });
            } else if meta.is_dir() {
                out.push(UploadItem::Dir(child.clone()));
                queue.push_back((path, child));
            } else if meta.is_file() {
                out.push(UploadItem::File { local: path, remote: child, size: meta.len() });
            }
        }
    }
    Ok(out)
}

async fn afc(h: &DeviceHandle) -> Result<AfcClient> {
    let stream = h.conn().lockdown_service(h.udid(), "com.apple.afc").await?;
    Ok(AfcClient::new(Idevice::new(stream, LABEL)))
}

async fn instproxy(h: &DeviceHandle) -> Result<InstallationProxyClient> {
    let stream = h.conn().lockdown_service(h.udid(), "com.apple.mobile.installation_proxy").await?;
    Ok(InstallationProxyClient::new(Idevice::new(stream, LABEL)))
}

async fn upload_file(afc: &mut AfcClient, bytes: &[u8], remote: &str) -> Result<()> {
    let mut fd =
        afc.open(remote, AfcFopenMode::WrOnly).await.map_err(|e| from_idevice(e, &format!("AFC open {remote}")))?;
    let res = fd.write_entire(bytes).await;
    let closed = fd.close().await;
    res.and(closed).map_err(|e| from_idevice(e, &format!("AFC write {remote}")))
}

/// Installs a `.app` directory or an `.ipa` (device.md §6.4).
pub async fn install(h: &DeviceHandle, pkg: &Path, o: InstallOptions, p: &dyn ProgressSink) -> Result<InstalledApp> {
    let started = Instant::now();
    let meta = std::fs::metadata(pkg).map_err(|e| invalid_argument(format!("{}: {e}", pkg.display())))?;
    let is_dir = meta.is_dir();
    let bundle = if is_dir { read_app_dir(pkg)? } else { read_ipa(pkg)? };
    p.begin(&format!("Installing {}", bundle.bundle_id), false);
    let mut afc = afc(h).await?;
    if afc.get_file_info(PUBLIC_STAGING).await.is_err() {
        afc.mk_dir(PUBLIC_STAGING).await.map_err(|e| from_idevice(e, "creating PublicStaging"))?;
    }
    let stage = format!("{PUBLIC_STAGING}/russet-{}", uuid::Uuid::new_v4().simple());
    let uploaded = Arc::new(AtomicU64::new(0));
    let (remote_package, mut options) = if is_dir {
        let remote = format!("{stage}.app");
        let items = plan_dir_upload(pkg, &remote)?;
        let total: u64 = items.iter().map(|i| if let UploadItem::File { size, .. } = i { *size } else { 0 }).sum();
        for item in items {
            match item {
                UploadItem::Dir(d) => {
                    if afc.get_file_info(d.as_str()).await.is_err() {
                        afc.mk_dir(d.as_str()).await.map_err(|e| from_idevice(e, &format!("AFC mkdir {d}")))?;
                    }
                }
                UploadItem::Symlink { target, remote } => {
                    afc.link(target.as_str(), remote.as_str(), LinkType::Symlink)
                        .await
                        .map_err(|e| from_idevice(e, &format!("AFC symlink {remote}")))?;
                }
                UploadItem::File { local, remote, size } => {
                    let bytes = tokio::fs::read(&local)
                        .await
                        .map_err(|e| invalid_argument(format!("{}: {e}", local.display())))?;
                    upload_file(&mut afc, &bytes, &remote).await?;
                    let done = uploaded.fetch_add(size, Ordering::SeqCst) + size;
                    if let Some(pct) = (done * 50).checked_div(total) {
                        p.report(Some("uploading"), Some(pct as u8));
                    }
                }
            }
        }
        let mut opts = plist::Dictionary::new();
        opts.insert("PackageType".into(), "Developer".into());
        (remote, opts)
    } else {
        let remote = format!("{stage}.ipa");
        let bytes = tokio::fs::read(pkg).await.map_err(|e| invalid_argument(format!("{}: {e}", pkg.display())))?;
        upload_file(&mut afc, &bytes, &remote).await?;
        uploaded.fetch_add(bytes.len() as u64, Ordering::SeqCst);
        p.report(Some("uploaded"), Some(50));
        let mut opts = plist::Dictionary::new();
        opts.insert("CFBundleIdentifier".into(), bundle.bundle_id.clone().into());
        (remote, opts)
    };
    for (k, v) in o.client_options {
        options.insert(k, v.into());
    }
    let mut ip = instproxy(h).await?;
    // installation_proxy reports its own 0–100; the upload was the first half.
    let progress_cb = |(pct, ()): (u64, ())| {
        p.report(Some("installing"), Some((50 + pct.min(100) / 2) as u8));
        std::future::ready(())
    };
    let res = ip
        .install_with_callback(remote_package.clone(), Some(plist::Value::Dictionary(options)), progress_cb, ())
        .await;
    // installd empties PublicStaging after an IPA; a developer directory may stay behind.
    let _ = afc.remove_all(remote_package.as_str()).await;
    res.map_err(|e| h.note_error(from_idevice(e, &format!("installing {}", bundle.bundle_id))))?;
    p.report(Some("installed"), Some(100));
    let installed_path = lookup(h, &bundle.bundle_id).await.ok().flatten().and_then(|a| a.path);
    p.end(Some("installed"));
    Ok(InstalledApp {
        bundle_id: bundle.bundle_id,
        installed_path,
        duration: started.elapsed(),
        bytes: uploaded.load(Ordering::SeqCst),
    })
}

fn app_from_lookup(bundle_id: &str, v: &plist::Value) -> AppInfo {
    let d = v.as_dictionary();
    let s = |k: &str| d.and_then(|d| d.get(k)).and_then(|v| v.as_string()).map(str::to_owned);
    let b = |k: &str| d.and_then(|d| d.get(k)).and_then(|v| v.as_boolean());
    let application_type = s("ApplicationType");
    AppInfo {
        bundle_id: bundle_id.to_owned(),
        name: s("CFBundleDisplayName").or_else(|| s("CFBundleName")),
        version: s("CFBundleShortVersionString"),
        build: s("CFBundleVersion"),
        path: s("Path"),
        executable: s("CFBundleExecutable"),
        developer_app: application_type.as_deref() == Some("User")
            && d.and_then(|d| d.get("Entitlements"))
                .and_then(|e| e.as_dictionary())
                .and_then(|e| e.get("get-task-allow"))
                .and_then(|v| v.as_boolean())
                .unwrap_or(false),
        removable: b("IsRemovable").unwrap_or(application_type.as_deref() == Some("User")),
        hidden: b("IsHidden").unwrap_or(false),
        application_type,
    }
}

/// `app.list` through `installation_proxy Lookup` (no tunnel and no DDI needed).
pub async fn list(h: &DeviceHandle, f: &AppFilter) -> Result<Vec<AppInfo>> {
    let mut ip = instproxy(h).await?;
    let kind = if f.all { "Any" } else { "User" };
    let apps = ip.get_apps(Some(kind), None).await.map_err(|e| from_idevice(e, "listing apps"))?;
    let mut out: Vec<AppInfo> = apps
        .iter()
        .filter(|(id, _)| f.bundle_id_prefix.as_deref().is_none_or(|p| id.starts_with(p)))
        .map(|(id, v)| app_from_lookup(id, v))
        .collect();
    out.sort_by(|a, b| a.bundle_id.cmp(&b.bundle_id));
    Ok(out)
}

/// One app by bundle id.
pub async fn lookup(h: &DeviceHandle, bundle_id: &str) -> Result<Option<AppInfo>> {
    let mut ip = instproxy(h).await?;
    let apps = ip
        .get_apps(Some("Any"), Some(vec![bundle_id.to_owned()]))
        .await
        .map_err(|e| from_idevice(e, "looking up the app"))?;
    Ok(apps.get(bundle_id).map(|v| app_from_lookup(bundle_id, v)))
}

/// `app.uninstall`: `installation_proxy Uninstall`, falling back to CoreDevice `uninstallapp`.
pub async fn uninstall(h: &DeviceHandle, bundle_id: &str) -> Result<()> {
    if lookup(h, bundle_id).await?.is_none() {
        return Err(device_error(codes::app::NOT_INSTALLED, format!("{bundle_id} is not installed")));
    }
    let mut ip = instproxy(h).await?;
    match ip.uninstall(bundle_id, None).await {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::debug!(error = %e, "installation_proxy uninstall failed; trying CoreDevice");
            let stream = h.conn().rsd_service(h.udid(), service::APP_SERVICE).await?;
            let mut cd = CoreDeviceClient::new(stream).await?;
            cd.invoke(feature::UNINSTALL_APP, uninstall_input(bundle_id))
                .await
                .map(|_| ())
                .map_err(|e| e.into_error(&format!("uninstalling {bundle_id}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_uploads_keep_symlinks_and_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = dir.path().join("Probe.app");
        std::fs::create_dir_all(app.join("Frameworks/A.framework/Versions/A")).expect("mkdir");
        std::fs::write(app.join("Info.plist"), b"x").expect("w");
        std::fs::write(app.join("Frameworks/A.framework/Versions/A/A"), b"binary").expect("w");
        #[cfg(unix)]
        std::os::unix::fs::symlink("Versions/A/A", app.join("Frameworks/A.framework/A")).expect("symlink");
        let plan = plan_dir_upload(&app, "PublicStaging/x.app").expect("plan");
        assert_eq!(plan[0], UploadItem::Dir("PublicStaging/x.app".into()));
        assert!(plan.contains(&UploadItem::File {
            local: app.join("Info.plist"),
            remote: "PublicStaging/x.app/Info.plist".into(),
            size: 1
        }));
        #[cfg(unix)]
        assert!(plan.contains(&UploadItem::Symlink {
            target: "Versions/A/A".into(),
            remote: "PublicStaging/x.app/Frameworks/A.framework/A".into()
        }));
        // Every directory comes before anything inside it.
        let pos = |r: &str| plan.iter().position(|i| matches!(i, UploadItem::Dir(d) if d == r)).expect("dir");
        let file =
            plan.iter().position(|i| matches!(i, UploadItem::File { remote, .. } if remote.ends_with("Versions/A/A")));
        assert!(pos("PublicStaging/x.app/Frameworks/A.framework/Versions/A") < file.expect("file"));
    }

    #[test]
    fn bundle_ids_are_read_from_apps_and_ipas() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = dir.path().join("Probe.app");
        std::fs::create_dir_all(&app).expect("mkdir");
        let mut d = plist::Dictionary::new();
        d.insert("CFBundleIdentifier".into(), "dev.russet.test.probe".into());
        plist::to_file_xml(app.join("Info.plist"), &d).expect("plist");
        assert_eq!(read_app_dir(&app).expect("app").bundle_id, "dev.russet.test.probe");

        let ipa = dir.path().join("Probe.ipa");
        let f = std::fs::File::create(&ipa).expect("ipa");
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        z.add_directory("Payload/Probe.app/", opts).expect("dir");
        z.start_file("Payload/Probe.app/Info.plist", opts).expect("file");
        let mut buf = Vec::new();
        plist::to_writer_xml(&mut buf, &d).expect("plist");
        std::io::Write::write_all(&mut z, &buf).expect("write");
        z.finish().expect("finish");
        let meta = read_ipa(&ipa).expect("ipa");
        assert_eq!((meta.bundle_id.as_str(), meta.app_name.as_str()), ("dev.russet.test.probe", "Probe.app"));
        assert!(read_ipa(&app.join("Info.plist")).is_err());
    }

    #[test]
    fn lookup_entries_map_to_app_info() {
        let mut ents = plist::Dictionary::new();
        ents.insert("get-task-allow".into(), true.into());
        let mut d = plist::Dictionary::new();
        d.insert("ApplicationType".into(), "User".into());
        d.insert("CFBundleName".into(), "Probe".into());
        d.insert("CFBundleExecutable".into(), "RussetProbe".into());
        d.insert("Path".into(), "/private/var/containers/Bundle/Application/X/Probe.app".into());
        d.insert("Entitlements".into(), ents.into());
        let a = app_from_lookup("dev.russet.test.probe", &d.into());
        assert!(a.developer_app && a.removable && !a.hidden);
        assert_eq!(a.executable.as_deref(), Some("RussetProbe"));
    }
}
