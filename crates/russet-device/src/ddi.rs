// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Developer Disk Images (ARCHITECTURE §5.3 `ddi`, §16.3) [D 62, D 82, D 89]. W4: status, `Dir`, mount records;
//! W10: `Imported`, `Pmd3Cache`, `Pmd3AutoMount`, `list_imported`.
//!
//! Every mount Russet performs writes a [`MountRecord`] (`state/devices/<udid>/ddi.json`). `ddi_status()`
//! reports the recorded source while the mounted cryptex version and product build still match the record,
//! otherwise `unknown`. Payload paths always come from the selected BuildManifest identity.
//!
//! **Status** (device.md §6.3, F6): `mobile_image_mounter` `CopyDevices` (the entry mounted at
//! `/System/Developer`) plus, on iOS 26.4 and later, cryptexd `copy-installed` (`com.apple.MobileAsset.DDI`).
//! `LookupImage` is never used: it answers `NotFound` on iOS 27.
//!
//! **Mount** (iOS 27, Cryptex1): the identity's payloads ([`idevice::cryptexd::Cryptex1Assets::load`] on
//! `<dir>/Restore`), a TSS ticket from Apple for the device's personalization identifiers and nonce, then cryptexd
//! `install` with Xcode's values, `persistence 2` and `nonce-persistence 1`. idevice 0.1.68 sends
//! `nonce_persistence` as `persistence` (`cryptexd.rs:273-274`, device.md U9), so Russet builds the install
//! request itself ([`cryptex_install_argv`]) on idevice's public RemoteXPC client. The personalized-DMG path for
//! iOS 17–26.x is W10's.

use std::path::{Path, PathBuf};

use idevice::Idevice;
use idevice::cryptexd::{Cryptex1Assets, CryptexdClient, InstalledCryptex};
use idevice::services::mobile_image_mounter::ImageMounter;
use idevice::xpc::{Dictionary, XPCObject};
use idevice::{ReadWrite, RemoteXpcClient};
use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::error::codes;
use russet_core::progress::ProgressSink;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use time::OffsetDateTime;

use crate::coredevice::service;
use crate::error::{Cause, device_error, from_idevice, render_idevice};
use crate::manager::DeviceHandle;
use crate::pairing::LABEL;

/// The DDI cryptex identifier.
pub const DDI_CRYPTEX: &str = "com.apple.MobileAsset.DDI";
/// Where the DDI mounts.
pub const DDI_MOUNT_PATH: &str = "/System/Developer";

/// Where a DDI comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdiSource {
    /// `data/ddi/<ddiBuild>/iOS`; newest first when `None`.
    Imported { ddi_build: Option<String> },
    /// A DDI root holding `Restore/` and `version.plist` (for example a copy of a Mac's
    /// `/Library/Developer/DeveloperDiskImages/iOS_DDI`); Russet loads `<dir>/Restore`, and also accepts a
    /// directory that directly contains `BuildManifest.plist` [D 82].
    Dir(PathBuf),
    /// pymobiledevice3's mirror-derived cache; needs the allow-mirror consent.
    Pmd3Cache,
    /// The opt-in pymobiledevice3 subprocess (third-party mirror).
    Pmd3AutoMount { allow_mirror: bool },
}

/// The image form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DdiKind {
    Legacy,
    PersonalizedDmg,
    Cryptex1,
}

/// Where a mounted DDI came from, per the mount record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DdiOrigin {
    Imported,
    Dir,
    Pmd3Cache,
    Pmd3AutoMount,
    Unknown,
}

/// `device.ddi.status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdiStatus {
    pub mounted: bool,
    pub kind: DdiKind,
    /// The cryptex version (`27.1.266.1`) for Cryptex1, else the CoreDevice version (`642.16`).
    pub version: Option<String>,
    /// The DDI's `ProductBuildVersion`, for example `27A266a`.
    pub product_build: Option<String>,
    pub source: DdiOrigin,
}

/// One imported DDI (`data/ddi/index.json`). W10 defines the remaining fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedDdi {
    pub ddi_build: String,
    pub path: PathBuf,
}

/// The last DDI mount Russet performed on a device (`state/devices/<udid>/ddi.json`) [D 89].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MountRecord {
    pub source: DdiOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub ddi_build: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cryptex_version: Option<String>,
    /// SHA-384 of the mounted image (lowercase hex).
    pub image_sha384: String,
    #[serde(with = "time::serde::rfc3339")]
    pub mounted_at: OffsetDateTime,
}

/// The result of [`DeviceHandle::ddi_check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdiCheck {
    pub ddi_build: Option<String>,
    pub image_sha384: String,
    /// The size of the IM4M ticket Apple signed.
    pub ticket_bytes: usize,
}

/// What `CopyDevices` says about the mounted developer image.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MountedImage {
    /// `DiskImageType`: `Personalized` or `Developer`.
    pub image_type: Option<String>,
    /// `PersonalizedImageVersion` (the CoreDevice version, for example `642.4`).
    pub core_device_version: Option<String>,
    /// `PersonalizedImageVersionInfo.ProductBuildVersion`.
    pub product_build: Option<String>,
}

/// Finds the developer image among `CopyDevices` entries (mounted at `/System/Developer`).
pub fn parse_mounted(entries: &[plist::Value]) -> Option<MountedImage> {
    entries.iter().filter_map(|e| e.as_dictionary()).find_map(|d| {
        let s = |k: &str| d.get(k).and_then(|v| v.as_string()).map(str::to_owned);
        let mounted = d.get("IsMounted").and_then(|v| v.as_boolean()).unwrap_or(true);
        let is_developer = s("MountPath").as_deref() == Some(DDI_MOUNT_PATH)
            || s("PersonalizedImageType").as_deref() == Some("DeveloperDiskImage")
            || s("DiskImageType").as_deref() == Some("Developer");
        (mounted && is_developer).then(|| MountedImage {
            image_type: s("DiskImageType"),
            core_device_version: s("PersonalizedImageVersion"),
            product_build: d
                .get("PersonalizedImageVersionInfo")
                .and_then(|v| v.as_dictionary())
                .and_then(|i| i.get("ProductBuildVersion"))
                .and_then(|v| v.as_string())
                .map(str::to_owned),
        })
    })
}

/// The DDI form expected for an iOS major version (17–26.x personalized DMG, 27+ Cryptex1).
pub fn expected_kind(ios_major: Option<u32>) -> DdiKind {
    match ios_major {
        Some(m) if m >= 27 => DdiKind::Cryptex1,
        Some(m) if m < 17 => DdiKind::Legacy,
        _ => DdiKind::PersonalizedDmg,
    }
}

/// Combines `CopyDevices`, the installed cryptexes and the mount record into a status [D 89].
pub fn status_from(
    mounted: Option<&MountedImage>,
    cryptexes: &[InstalledCryptex],
    record: Option<&MountRecord>,
    ios_major: Option<u32>,
) -> DdiStatus {
    let cryptex = cryptexes.iter().find(|c| c.identifier == DDI_CRYPTEX);
    let Some(m) = mounted else {
        return DdiStatus {
            mounted: false,
            kind: expected_kind(ios_major),
            version: cryptex.map(|c| c.version.clone()),
            product_build: None,
            source: DdiOrigin::Unknown,
        };
    };
    let kind = if cryptex.is_some() {
        DdiKind::Cryptex1
    } else if m.image_type.as_deref() == Some("Developer") {
        DdiKind::Legacy
    } else {
        DdiKind::PersonalizedDmg
    };
    let version = match kind {
        DdiKind::Cryptex1 => cryptex.map(|c| c.version.clone()),
        _ => m.core_device_version.clone(),
    };
    let mut status =
        DdiStatus { mounted: true, kind, version, product_build: m.product_build.clone(), source: DdiOrigin::Unknown };
    status.source = attribute(&status, record);
    status
}

/// The recorded source while the mounted image still matches the record, else `unknown` [D 89]. Matching
/// versions alone are not enough without a record: a directory copy and an import of the same Xcode are the same
/// bytes.
pub fn attribute(status: &DdiStatus, record: Option<&MountRecord>) -> DdiOrigin {
    match record {
        Some(r)
            if status.mounted
                && status.product_build.as_deref() == Some(r.ddi_build.as_str())
                && (r.cryptex_version.is_none() || status.version == r.cryptex_version) =>
        {
            r.source
        }
        _ => DdiOrigin::Unknown,
    }
}

/// `<dir>/Restore` when `dir` is a DDI root, or `dir` itself when it directly holds `BuildManifest.plist` [D 82].
pub fn resolve_restore_dir(dir: &Path) -> Result<PathBuf> {
    let restore = dir.join("Restore");
    if restore.join("BuildManifest.plist").is_file() {
        return Ok(restore);
    }
    if dir.join("BuildManifest.plist").is_file() {
        return Ok(dir.to_path_buf());
    }
    Err(device_error(
        codes::device::DDI_SOURCE_MISSING,
        format!("{} holds neither Restore/BuildManifest.plist nor BuildManifest.plist", dir.display()),
    ))
}

/// The DDI's `ProductBuildVersion`: `version.plist` next to `Restore/`, else the BuildManifest's.
pub fn ddi_build(dir: &Path, restore: &Path) -> Option<String> {
    let read = |p: &Path| -> Option<String> {
        let v: plist::Value = plist::from_file(p).ok()?;
        v.as_dictionary()?.get("ProductBuildVersion")?.as_string().map(str::to_owned)
    };
    read(&dir.join("version.plist")).or_else(|| read(&restore.join("BuildManifest.plist")))
}

/// Lowercase hex SHA-384.
pub fn sha384_hex(bytes: &[u8]) -> String {
    sha2::Sha384::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The cryptexd `install` arguments with Xcode's values (device.md U9): `persistence` 2 and `nonce-persistence` 1,
/// each sent under its own key. The five payloads are announced as file transfers `1…5`.
pub fn cryptex_install_argv(
    cryptex1_properties: Dictionary,
    sizes: [u64; 5],
    persistence: u64,
    nonce_persistence: u64,
) -> Dictionary {
    let mut argv = Dictionary::new();
    argv.insert("auth".into(), XPCObject::UInt64(0));
    argv.insert("client-version".into(), XPCObject::UInt64(idevice::cryptexd::CLIENT_VERSION));
    argv.insert("cryptex1-properties".into(), XPCObject::Dictionary(cryptex1_properties));
    argv.insert("image-type-index".into(), XPCObject::Int64(idevice::cryptexd::DDI_IMAGE_TYPE_INDEX));
    argv.insert("nonce-persistence".into(), XPCObject::UInt64(nonce_persistence));
    argv.insert("persistence".into(), XPCObject::UInt64(persistence));
    for (i, (key, size)) in TRANSFER_KEYS.iter().zip(sizes).enumerate() {
        let mut s = Dictionary::new();
        s.insert("s".into(), XPCObject::UInt64(size));
        argv.insert(
            (*key).to_owned(),
            XPCObject::FileTransfer { msg_id: i as u64 + 1, data: Box::new(XPCObject::Dictionary(s)) },
        );
    }
    argv
}

/// The file-transfer keys of a cryptex install, in transfer order.
pub const TRANSFER_KEYS: [&str; 5] = ["image", "trustcache", "im4m", "info", "volumehash"];

/// The full `install` routine message.
pub fn cryptex_install_request(argv: Dictionary) -> Dictionary {
    let mut req = Dictionary::new();
    req.insert("routine".into(), XPCObject::String("install".into()));
    req.insert("argv".into(), XPCObject::Dictionary(argv));
    req
}

/// cryptexd's reply check (idevice's `unwrap_reply` is private): `cferr` or a non-zero `error` fail.
fn check_cryptexd_reply(routine: &str, reply: &plist::Value) -> Result<()> {
    let d = reply.as_dictionary();
    if let Some(cferr) = d.and_then(|d| d.get("cferr")).and_then(|v| v.as_dictionary()) {
        let desc = cferr
            .get("cferr_userinfo")
            .and_then(|v| v.as_dictionary())
            .and_then(|u| u.get("NSLocalizedDescription"))
            .and_then(|v| v.as_string())
            .unwrap_or("cryptexd refused the request");
        let domain = cferr.get("cferr_domain").and_then(|v| v.as_string()).unwrap_or("?");
        let code = cferr.get("cferr_code").and_then(|v| v.as_signed_integer()).unwrap_or_default();
        return Err(device_error(
            codes::device::DDI_PERSONALIZATION_FAILED,
            format!("cryptexd {routine}: {desc} ({domain} {code})"),
        )
        .with_details(serde_json::json!({ "domain": domain, "code": code })));
    }
    let error = d.and_then(|d| d.get("error")).and_then(|v| v.as_signed_integer()).unwrap_or(0);
    if error != 0 {
        return Err(device_error(
            codes::device::DDI_PERSONALIZATION_FAILED,
            format!("cryptexd {routine}: {}", idevice::darwin_errno::describe_errno(error)),
        )
        .with_details(serde_json::json!({ "errno": error })));
    }
    Ok(())
}

/// The Cryptex1 install with the persistence fix, on one cryptexd connection.
pub async fn install_cryptex(stream: Box<dyn ReadWrite>, assets: &Cryptex1Assets, im4m: Vec<u8>) -> Result<()> {
    let properties = assets.cryptex1_properties().map_err(|e| from_idevice(e, "reading the cryptex identity"))?;
    let payloads: [&[u8]; 5] = [&assets.image, &assets.trustcache, &im4m, &assets.info, &assets.volumehash];
    let sizes = payloads.map(|p| p.len() as u64);
    let argv = cryptex_install_argv(
        properties,
        sizes,
        idevice::cryptexd::DDI_PERSISTENCE,
        idevice::cryptexd::DDI_NONCE_PERSISTENCE,
    );
    let mut xpc = RemoteXpcClient::new(stream).await.map_err(|e| from_idevice(e, "cryptexd connect"))?;
    xpc.do_handshake().await.map_err(|e| from_idevice(e, "cryptexd handshake"))?;
    xpc.send_object(XPCObject::Dictionary(cryptex_install_request(argv)), true)
        .await
        .map_err(|e| from_idevice(e, "cryptexd install"))?;
    for (i, payload) in payloads.iter().enumerate() {
        xpc.send_file_transfer(i as u64 + 1, payload)
            .await
            .map_err(|e| from_idevice(e, &format!("uploading the cryptex {}", TRANSFER_KEYS[i])))?;
    }
    let reply = xpc.recv().await.map_err(|e| from_idevice(e, "cryptexd install reply"))?;
    check_cryptexd_reply("install", &reply)
}

impl DeviceHandle {
    fn mount_record_path(&self) -> PathBuf {
        self.cfg().device_state_dir(self.udid().as_str()).join("ddi.json")
    }

    /// The last mount record, if any.
    pub fn mount_record(&self) -> Option<MountRecord> {
        let bytes = std::fs::read(self.mount_record_path()).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn write_mount_record(&self, r: &MountRecord) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(r).map_err(|e| {
            russet_core::Error::new(codes::core::INTERNAL, format!("cannot encode the mount record: {e}"))
        })?;
        crate::config::write_atomic(&self.mount_record_path(), &bytes)
    }

    async fn copy_devices(&self) -> Result<Vec<plist::Value>> {
        let stream = self.conn().lockdown_service(self.udid(), "com.apple.mobile.mobile_image_mounter").await?;
        let mut mounter = ImageMounter::new(Idevice::new(stream, LABEL));
        mounter.copy_devices().await.map_err(|e| from_idevice(e, "image mounter CopyDevices"))
    }

    async fn cryptexd(&self) -> Result<CryptexdClient<Box<dyn ReadWrite>>> {
        let stream = self.conn().rsd_service(self.udid(), service::CRYPTEXD).await?;
        CryptexdClient::new(stream).await.map_err(|e| from_idevice(e, "cryptexd connect"))
    }

    async fn installed_cryptexes(&self) -> Result<Vec<InstalledCryptex>> {
        self.cryptexd().await?.copy_installed().await.map_err(|e| from_idevice(e, "cryptexd copy-installed"))
    }

    fn ios_major(&self) -> Option<u32> {
        self.actor.cached().identity.and_then(|i| i.ios_major())
    }

    pub async fn ddi_status(&self) -> Result<DdiStatus> {
        if self.ios_major().is_none() {
            let _ = self.ensure_identity().await;
        }
        let major = self.ios_major();
        let entries = self.copy_devices().await?;
        let mounted = parse_mounted(&entries);
        let cryptexes = if major.is_none_or(|m| m >= 26) {
            match self.installed_cryptexes().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e.message(), "cryptexd copy-installed failed");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let record = self.mount_record();
        Ok(status_from(mounted.as_ref(), &cryptexes, record.as_ref(), major))
    }

    pub async fn ddi_mount(&self, src: &DdiSource, force: bool, p: &dyn ProgressSink) -> Result<DdiStatus> {
        let _guard = self.actor.device_lock.lock().await;
        let dir = match src {
            DdiSource::Dir(dir) => dir.clone(),
            DdiSource::Imported { .. } => {
                return Err(device_error(codes::device::DDI_SOURCE_MISSING, "imported DDIs arrive with W10 (Wave 2)")
                    .with_hint("Set `[device.ddi] dir` to a copy of a Mac's iOS_DDI folder for now."));
            }
            DdiSource::Pmd3Cache | DdiSource::Pmd3AutoMount { .. } => {
                if !self.current_ddi_config().allow_mirror {
                    return Err(device_error(
                        codes::device::DDI_MIRROR_CONSENT_REQUIRED,
                        "pymobiledevice3's DDIs come from a third-party mirror",
                    ));
                }
                return Err(device_error(
                    codes::device::FALLBACK_UNAVAILABLE,
                    "the pymobiledevice3 DDI sources arrive with W10 (Wave 2)",
                ));
            }
        };
        let status = self.ddi_status().await?;
        if status.mounted && !force {
            return Ok(status);
        }
        let major = self.ios_major();
        if expected_kind(major) != DdiKind::Cryptex1 {
            return Err(device_error(
                codes::device::UNSUPPORTED,
                "mounting a personalized DMG (iOS 17–26) arrives with W10; this build mounts Cryptex1 DDIs (iOS 27+)",
            ));
        }
        p.begin("Mounting the Developer Disk Image", false);
        let restore = resolve_restore_dir(&dir)?;
        let build = ddi_build(&dir, &restore);
        p.report(Some("reading the DDI"), Some(5));
        let assets = Cryptex1Assets::load(&restore).await.map_err(|e| {
            device_error(codes::device::DDI_SOURCE_MISSING, format!("{}: {}", restore.display(), render_idevice(&e)))
                .caused_by(Cause(render_idevice(&e)))
        })?;
        p.report(Some("personalizing with Apple (TSS)"), Some(20));
        let im4m = self.personalize(&assets).await?;
        p.report(Some("installing the cryptex"), Some(50));
        install_cryptex(self.conn().rsd_service(self.udid(), service::CRYPTEXD).await?, &assets, im4m)
            .await
            .map_err(|e| self.note_error(e))?;
        let cryptex_version = self
            .installed_cryptexes()
            .await
            .ok()
            .and_then(|c| c.into_iter().find(|c| c.identifier == DDI_CRYPTEX))
            .map(|c| c.version);
        let record = MountRecord {
            source: DdiOrigin::Dir,
            path: Some(dir.clone()),
            ddi_build: build.unwrap_or_default(),
            cryptex_version,
            image_sha384: sha384_hex(&assets.image),
            mounted_at: OffsetDateTime::now_utc(),
        };
        self.write_mount_record(&record)?;
        p.end(Some("mounted"));
        let status = self.ddi_status().await?;
        self.actor.update(|c| {
            if let Some(r) = c.readiness.as_mut() {
                r.ddi = Some(status.clone());
            }
        });
        self.changed();
        Ok(status)
    }

    /// Checks that a DDI directory fits this device without installing anything: loads the cryptex identity's
    /// payloads and has Apple's TSS personalize them for the device's identifiers and current nonce (read, not
    /// rolled). The dry run of [`Self::ddi_mount`]; nothing on the device changes.
    pub async fn ddi_check(&self, dir: &Path) -> Result<DdiCheck> {
        if expected_kind(self.ios_major()) != DdiKind::Cryptex1 {
            let _ = self.ensure_identity().await;
        }
        let restore = resolve_restore_dir(dir)?;
        let assets = Cryptex1Assets::load(&restore).await.map_err(|e| {
            device_error(codes::device::DDI_SOURCE_MISSING, format!("{}: {}", restore.display(), render_idevice(&e)))
        })?;
        let ticket = self.personalize(&assets).await?;
        Ok(DdiCheck {
            ddi_build: ddi_build(dir, &restore),
            image_sha384: sha384_hex(&assets.image),
            ticket_bytes: ticket.len(),
        })
    }

    /// A Cryptex1 ticket from Apple's TSS for this device's personalization identifiers and nonce.
    async fn personalize(&self, assets: &Cryptex1Assets) -> Result<Vec<u8>> {
        let chip = self
            .cryptexd()
            .await?
            .read_personalization_identifiers()
            .await
            .map_err(|e| from_idevice(e, "cryptexd read-personalization-id"))?;
        let domain = assets.nonce_domain().map_err(|e| from_idevice(e, "the cryptex nonce domain"))?;
        let nonce =
            self.cryptexd().await?.cryptex_nonce(domain).await.map_err(|e| from_idevice(e, "cryptexd get-nonce"))?;
        let mut req = idevice::tss::TSSRequest::new();
        req.add_cryptex1_tags(&assets.build_identity, &chip, &nonce)
            .map_err(|e| from_idevice(e, "building the TSS request"))?;
        let resp = req.send().await.map_err(|e| {
            device_error(codes::device::DDI_PERSONALIZATION_FAILED, format!("TSS: {}", render_idevice(&e)))
                .caused_by(Cause(render_idevice(&e)))
        })?;
        let resp = resp.as_dictionary().ok_or_else(|| {
            device_error(codes::device::DDI_PERSONALIZATION_FAILED, "TSS answered something other than a dictionary")
        })?;
        resp.get("Cryptex1,Ticket")
            .or_else(|| resp.get("ApImg4Ticket"))
            .and_then(|v| v.as_data())
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                device_error(
                    codes::device::DDI_INCOMPATIBLE,
                    "TSS did not sign a Cryptex1 ticket for this device and DDI",
                )
            })
    }

    /// `[device.ddi]` as the config file says now (`russet config set device.ddi.dir …` applies without a daemon
    /// restart), else the settings the manager started with.
    pub fn current_ddi_config(&self) -> crate::config::DdiConfig {
        let cfg = self.cfg();
        if cfg.config_dir.as_os_str().is_empty() {
            return cfg.ddi.clone();
        }
        let path = cfg.config_dir.join("config.toml");
        let mut fresh = crate::config::DeviceConfig { ddi: cfg.ddi.clone(), ..Default::default() };
        match std::fs::read_to_string(&path) {
            Ok(text) => match fresh.apply_toml(&text, &path) {
                Ok(()) => fresh.ddi,
                Err(e) => {
                    tracing::warn!(error = %e.message(), "config.toml is invalid; using the DDI settings from start-up");
                    cfg.ddi.clone()
                }
            },
            Err(_) => cfg.ddi.clone(),
        }
    }

    /// The `[device.ddi]` order of ARCHITECTURE §14.3; lazy; remounts only when missing.
    pub async fn ensure_ddi(&self) -> Result<DdiStatus> {
        let status = self.ddi_status().await?;
        if status.mounted {
            return Ok(status);
        }
        let progress = crate::NoProgress;
        let mut last_err = None;
        let ddi = self.current_ddi_config();
        for source in &ddi.order {
            let src = match source.as_str() {
                "dir" => match &ddi.dir {
                    Some(d) => DdiSource::Dir(d.clone()),
                    None => continue,
                },
                // W10: imported DDIs and the pymobiledevice3 sources.
                _ => continue,
            };
            match self.ddi_mount(&src, false, &progress).await {
                Ok(s) => return Ok(s),
                Err(e)
                    if e.code() == codes::device::DDI_INCOMPATIBLE
                        || e.code() == codes::device::DDI_PERSONALIZATION_FAILED =>
                {
                    last_err = Some(e)
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            device_error(
                codes::device::DDI_SOURCE_MISSING,
                "no DDI is mounted and `[device.ddi]` names no usable source",
            )
        }))
    }
}

/// `data/ddi/index.json`, newest `ddiBuild` first (`device.ddi.list`). W10 writes the index (W16b's importer); this
/// reads the entries that exist and returns none when there is no index.
pub fn list_imported(dirs: &RussetDirs) -> Result<Vec<ImportedDdi>> {
    let index = dirs.data.join("ddi").join("index.json");
    let bytes = match std::fs::read(&index) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(
                russet_core::Error::new(codes::core::IO, format!("cannot read {}: {e}", index.display())).caused_by(e)
            );
        }
    };
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Entry {
        ddi_build: String,
    }
    let entries: Vec<Entry> = serde_json::from_slice(&bytes)
        .map_err(|e| russet_core::Error::new(codes::sdk::IMPORT_FAILED, format!("{}: {e}", index.display())))?;
    let mut out: Vec<ImportedDdi> = entries
        .into_iter()
        .map(|e| ImportedDdi { path: dirs.data.join("ddi").join(&e.ddi_build).join("iOS"), ddi_build: e.ddi_build })
        .collect();
    out.sort_by(|a, b| b.ddi_build.cmp(&a.ddi_build));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CopyDevices` as the rig answered it on 2026-10-11 with the spike's mirror-sourced DDI (no personal data).
    const COPY_DEVICES_27A5228H: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>BackingImage</key><string>/private/var/db/com.apple.security.cryptexd/codex.system/cryptex/com.apple.MobileAsset.DDI/gdmg</string>
  <key>DeviceNode</key><string>/dev/disk4s1</string>
  <key>DeviceType</key><string>DiskImage</string>
  <key>DiskImageType</key><string>Personalized</string>
  <key>FilesystemType</key><string>apfs</string>
  <key>IsMounted</key><true/>
  <key>IsReadOnly</key><true/>
  <key>MountPath</key><string>/System/Developer</string>
  <key>PersonalizedImageType</key><string>DeveloperDiskImage</string>
  <key>PersonalizedImageVersion</key><string>642.4</string>
  <key>PersonalizedImageVersionInfo</key><dict>
    <key>BuildVersion</key><string>760</string>
    <key>Platform</key><string>iOS</string>
    <key>ProductBuildVersion</key><string>27A5228h</string>
    <key>ProjectMetadata</key><array><dict><key>Project</key><string>CoreDevice</string><key>Version</key><string>642.4</string></dict></array>
  </dict>
</dict></plist>"#;

    fn entries() -> Vec<plist::Value> {
        vec![plist::from_bytes(COPY_DEVICES_27A5228H.as_bytes()).expect("fixture parses")]
    }

    fn ddi_cryptex(version: &str) -> Vec<InstalledCryptex> {
        vec![InstalledCryptex { identifier: DDI_CRYPTEX.into(), version: version.into() }]
    }

    fn record(source: DdiOrigin, build: &str, cryptex: &str) -> MountRecord {
        MountRecord {
            source,
            path: Some(PathBuf::from("/srv/ddi/iOS")),
            ddi_build: build.into(),
            cryptex_version: Some(cryptex.into()),
            image_sha384: "00".repeat(48),
            mounted_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn status_parses_the_recorded_copy_devices_and_cryptexd_replies() {
        let m = parse_mounted(&entries()).expect("a developer image is mounted");
        assert_eq!(m.product_build.as_deref(), Some("27A5228h"));
        assert_eq!(m.core_device_version.as_deref(), Some("642.4"));
        let s = status_from(Some(&m), &ddi_cryptex("27.1.5228.8"), None, Some(27));
        assert_eq!(
            s,
            DdiStatus {
                mounted: true,
                kind: DdiKind::Cryptex1,
                version: Some("27.1.5228.8".into()),
                product_build: Some("27A5228h".into()),
                // The spike's mount has no Russet mount record (W4 acceptance: `source: unknown`).
                source: DdiOrigin::Unknown,
            }
        );
        // Without cryptexd (iOS 17–26) the same entry is a personalized DMG with the CoreDevice version.
        let s = status_from(Some(&m), &[], None, Some(26));
        assert_eq!((s.kind, s.version.as_deref()), (DdiKind::PersonalizedDmg, Some("642.4")));
        // Nothing mounted.
        let s = status_from(None, &[], None, Some(27));
        assert_eq!((s.mounted, s.kind, s.source), (false, DdiKind::Cryptex1, DdiOrigin::Unknown));
    }

    #[test]
    fn mount_records_attribute_only_while_they_match() {
        let m = parse_mounted(&entries()).expect("mounted");
        let ours = record(DdiOrigin::Dir, "27A5228h", "27.1.5228.8");
        assert_eq!(status_from(Some(&m), &ddi_cryptex("27.1.5228.8"), Some(&ours), Some(27)).source, DdiOrigin::Dir);
        // Another DDI was mounted since Russet's mount (for example after a reboot): unknown.
        let older = record(DdiOrigin::Dir, "27A266a", "27.1.266.1");
        assert_eq!(
            status_from(Some(&m), &ddi_cryptex("27.1.5228.8"), Some(&older), Some(27)).source,
            DdiOrigin::Unknown
        );
        // The record keeps its source (an import of the same bytes would say `imported`).
        let imported = record(DdiOrigin::Imported, "27A5228h", "27.1.5228.8");
        assert_eq!(
            status_from(Some(&m), &ddi_cryptex("27.1.5228.8"), Some(&imported), Some(27)).source,
            DdiOrigin::Imported
        );
        // Records round-trip in the §14.8 shape.
        let json = serde_json::to_value(&ours).expect("encodes");
        assert_eq!(json["source"], "dir");
        assert_eq!(json["ddiBuild"], "27A5228h");
        assert_eq!(json["mountedAt"], "1970-01-01T00:00:00Z");
        let back: MountRecord = serde_json::from_value(json).expect("decodes");
        assert_eq!(back, ours);
    }

    #[test]
    fn the_install_request_carries_xcode_persistence_values() {
        let mut props = Dictionary::new();
        props.insert("Cryptex1,NonceDomain".into(), XPCObject::UInt64(4));
        let argv = cryptex_install_argv(
            props,
            [15_895_040, 1895, 4096, 430, 229],
            idevice::cryptexd::DDI_PERSISTENCE,
            idevice::cryptexd::DDI_NONCE_PERSISTENCE,
        );
        // Encode with idevice's XPC codec (the bytes that go on the wire) and decode them back.
        let wire = XPCObject::Dictionary(cryptex_install_request(argv)).encode().expect("encodes");
        let decoded = XPCObject::decode(&wire).expect("decodes").to_plist();
        let argv = decoded.as_dictionary().and_then(|d| d.get("argv")).and_then(|a| a.as_dictionary()).expect("argv");
        assert_eq!(argv.get("persistence").and_then(|v| v.as_unsigned_integer()), Some(2));
        assert_eq!(argv.get("nonce-persistence").and_then(|v| v.as_unsigned_integer()), Some(1));
        assert_eq!(argv.get("image-type-index").and_then(|v| v.as_signed_integer()), Some(10));
        for key in TRANSFER_KEYS {
            assert!(argv.contains_key(key), "{key} missing");
        }
    }

    #[test]
    fn cryptexd_errors_are_personalization_failures() {
        let mut d = plist::Dictionary::new();
        d.insert("error".into(), 17.into());
        let e = check_cryptexd_reply("install", &d.into()).expect_err("errno 17");
        assert_eq!(e.code(), codes::device::DDI_PERSONALIZATION_FAILED);
        let mut ok = plist::Dictionary::new();
        ok.insert("error".into(), 0.into());
        check_cryptexd_reply("install", &ok.into()).expect("ok");
    }

    #[test]
    fn imported_index_is_optional() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dirs = RussetDirs::under(dir.path());
        assert!(list_imported(&dirs).expect("no index").is_empty());
        std::fs::create_dir_all(dirs.data.join("ddi")).expect("mkdir");
        std::fs::write(dirs.data.join("ddi/index.json"), r#"[{"ddiBuild":"27A266a"},{"ddiBuild":"27B100"}]"#)
            .expect("w");
        let list = list_imported(&dirs).expect("index");
        assert_eq!(list[0].ddi_build, "27B100");
        assert!(list[1].path.ends_with("ddi/27A266a/iOS"));
    }
}
